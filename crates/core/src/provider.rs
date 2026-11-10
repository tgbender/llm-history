//! Shared timestamp interpretation.
use super::*;
use chrono::Offset;
use llm_history_types::provider::Timestamp;

pub fn timestamp(raw: &str, hint: &str) -> Timestamp {
    let raw = raw.trim();
    if raw.is_empty() {
        return Timestamp {
            basis: "missing".into(),
            ..Default::default()
        };
    }
    let normalized = if let Some(prefix) = raw.strip_suffix('z') {
        format!("{prefix}Z")
    } else {
        raw.into()
    };
    if let Ok(time) = DateTime::parse_from_rfc3339(&normalized) {
        let offset = time.offset().local_minus_utc() / 60;
        return Timestamp {
            utc: Some(time.with_timezone(&Utc).to_rfc3339()),
            offset_minutes: Some(offset),
            timezone: if offset == 0 {
                "UTC".into()
            } else {
                time.format("%:z").to_string()
            },
            basis: if offset == 0 {
                "explicit_utc"
            } else {
                "explicit_offset"
            }
            .into(),
        };
    }
    let naive = parse_naive(raw);
    let Some(naive) = naive else {
        return Timestamp {
            timezone: hint.into(),
            basis: "invalid".into(),
            ..Default::default()
        };
    };
    if hint.is_empty() {
        return Timestamp {
            basis: "naive_unresolved".into(),
            ..Default::default()
        };
    }
    let Ok(zone) = hint.parse::<chrono_tz::Tz>() else {
        return Timestamp {
            timezone: hint.into(),
            basis: "invalid_timezone".into(),
            ..Default::default()
        };
    };
    let resolved = zone
        .from_local_datetime(&naive)
        .earliest()
        .map(|time| time.fixed_offset())
        .or_else(|| {
            // Match ZoneInfo's fold=0 treatment of a spring-forward gap.
            zone.from_local_datetime(&(naive - chrono::Duration::hours(3)))
                .earliest()
                .and_then(|before| before.offset().fix().from_local_datetime(&naive).single())
        });
    match resolved {
        Some(time) => Timestamp {
            utc: Some(time.with_timezone(&Utc).to_rfc3339()),
            offset_minutes: Some(time.offset().local_minus_utc() / 60),
            timezone: hint.into(),
            basis: "source_timezone".into(),
        },
        None => Timestamp {
            timezone: hint.into(),
            basis: "invalid".into(),
            ..Default::default()
        },
    }
}
pub(crate) fn parse_naive(raw: &str) -> Option<NaiveDateTime> {
    ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(raw, format).ok())
        .or_else(|| {
            NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .ok()
                .and_then(|date| date.and_hms_opt(0, 0, 0))
        })
}
