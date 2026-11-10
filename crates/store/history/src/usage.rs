//! Deduplicated token usage by period, profile and model.
use super::*;
use duckdb::types::Value as SqlValue;
use llm_history_types::query::UsagePeriod;

pub(crate) fn events_query(scoped: bool) -> String {
    include_str!("usage_events.sql").replace(
        "/* usage scope */",
        if scoped {
            "AND s.profile_id IN (SELECT id FROM selected)"
        } else {
            ""
        },
    )
}

impl Store {
    pub fn usage(
        &self,
        filter: &Filter,
        zone: &str,
        weekly: bool,
        by_model: bool,
        limit: usize,
    ) -> Result<Vec<UsagePeriod>> {
        let profiles: Vec<_> = self
            .profiles(&filter.profiles, false)?
            .into_iter()
            .filter(|p| {
                filter
                    .source
                    .as_deref()
                    .is_none_or(|source| source == "all" || source == p.harness.key())
            })
            .collect();
        // Existing usage filters compare millisecond-precision timestamps.
        // Convert the inclusive bounds into a native timestamp range once.
        let bound = |value: &Option<String>, upper: bool| {
            value
                .as_deref()
                .map(|value| {
                    let time = chrono::DateTime::parse_from_rfc3339(value)?;
                    let next = upper || time.timestamp_subsec_nanos() % 1_000_000 != 0;
                    Ok::<_, chrono::ParseError>((time.timestamp_millis() + i64::from(next)) * 1_000)
                })
                .transpose()
        };
        let (lower, upper) = (bound(&filter.since, false)?, bound(&filter.until, true)?);
        if profiles.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let zone = if zone == "local" {
            std::env::var("TZ")
                .ok()
                .and_then(|value| value.trim_start_matches(':').parse::<chrono_tz::Tz>().ok())
                .map(|zone| zone.name().to_owned())
                .map_or_else(iana_time_zone::get_timezone, Ok)?
        } else {
            zone.parse::<chrono_tz::Tz>()
                .context("invalid IANA timezone")?
                .name()
                .to_owned()
        };
        let ids = SqlValue::List(
            profiles
                .iter()
                .map(|p| SqlValue::Text(p.id.clone()))
                .collect(),
        );
        let names = SqlValue::List(
            profiles
                .into_iter()
                .map(|p| SqlValue::Text(p.name))
                .collect(),
        );
        let unit = if weekly { "week" } else { "day" };
        // A profile filter can prune whole deduplication partitions; time bounds
        // apply to the winners in usage_events, not the competing token rows.
        let events = events_query(true);
        let mut statement = self.database.connection().prepare(&format!(
            "WITH selected AS (SELECT unnest(?::VARCHAR[]) id, unnest(?::VARCHAR[]) name),
             scoped AS (
                SELECT date_trunc(?,timezone(?,u.timestamp_utc))::DATE::VARCHAR period,
                       p.name profile, CASE WHEN ? THEN u.model END model,
                       CASE WHEN ? THEN u.effort END effort,
                       u.input_tokens,u.output_tokens,u.cached_input_tokens,
                       u.cache_write_input_tokens,u.thinking_tokens
                FROM ({events}) u JOIN selected p ON p.id=u.profile_id
                WHERE u.timestamp_utc IS NOT NULL
                  AND u.timestamp_utc >= coalesce(make_timestamptz(?::BIGINT),'-infinity'::TIMESTAMPTZ)
                  AND u.timestamp_utc < coalesce(make_timestamptz(?::BIGINT),'infinity'::TIMESTAMPTZ)
             )
             SELECT period,profile,model,effort,
                    sum(input_tokens)::BIGINT,sum(output_tokens)::BIGINT,
                    sum(cached_input_tokens)::BIGINT,sum(cache_write_input_tokens)::BIGINT,
                    sum(thinking_tokens)::BIGINT
             FROM scoped GROUP BY 1,2,3,4 ORDER BY 1,2,3,4 LIMIT ?",
        ))?;
        let mut rows = statement.query(params![
            ids,
            names,
            unit,
            zone,
            by_model,
            by_model,
            lower,
            upper,
            limit.min(i64::MAX as usize) as i64
        ])?;
        let mut periods = Vec::new();
        while let Some(row) = rows.next()? {
            let input: i64 = row.get(4)?;
            let output: i64 = row.get(5)?;
            let cached: i64 = row.get(6)?;
            let cache_write: i64 = row.get(7)?;
            periods.push(UsagePeriod {
                period: row.get(0)?,
                profile: row.get(1)?,
                model: row.get(2)?,
                effort: row.get(3)?,
                input,
                output,
                cached,
                cache_write,
                thinking: row.get(8)?,
                total: input
                    .checked_add(output)
                    .context("token aggregate overflow")?,
                uncached: input
                    .saturating_sub(cached)
                    .saturating_sub(cache_write)
                    .max(0),
            });
        }
        Ok(periods)
    }
}
