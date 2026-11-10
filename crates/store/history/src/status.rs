//! Database coverage, entity counts and timestamp ranges.
use super::*;

impl Store {
    pub fn status(
        &self,
        names: &[String],
        source: Option<&str>,
        decoder_revision: u32,
    ) -> Result<Value> {
        if let Some(source) = source.filter(|s| *s != "all") {
            let _: Harness = source.parse()?;
        }
        let profiles = self
            .profiles(names, false)?
            .into_iter()
            .filter(|p| source.is_none_or(|s| s == "all" || s == p.harness.key()))
            .collect::<Vec<_>>();
        let ids = profiles
            .iter()
            .map(|p| format!("'{}'", p.id.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(",");
        let ids = if ids.is_empty() { "NULL" } else { &ids };
        let mut out = json!({"schema":self.database.connection().query_row("SELECT value FROM meta WHERE key='schema_version'",[],|r|r.get::<_,String>(0))?,"parser_version":decoder_revision,"identity":self.identity()?,"duckdb":self.database.connection().query_row("SELECT version()",[],|r|r.get::<_,String>(0))?,"profiles":profiles.len()});
        let sessions = format!("SELECT id FROM sessions WHERE profile_id IN ({ids})");
        let canonical = format!("SELECT id FROM canonical_sessions WHERE store_id IN ({ids})");
        let sources = format!("SELECT id FROM sources WHERE profile_id IN ({ids})");
        let occurrences = format!("SELECT id FROM events WHERE session_id IN ({sessions})");
        for (table, predicate) in [
            ("sources", format!("profile_id IN ({ids})")),
            ("generations", format!("source_id IN ({sources})")),
            ("sessions", format!("id IN ({sessions})")),
            ("events", format!("session_id IN ({sessions})")),
            ("embeddings", format!("message_id IN ({occurrences})")),
            (
                "embedding_failures",
                format!("message_id IN ({occurrences})"),
            ),
            ("canonical_sessions", format!("id IN ({canonical})")),
            ("canonical_events", format!("session_id IN ({canonical})")),
            ("canonical_messages", format!("session_id IN ({canonical})")),
            (
                "canonical_tool_calls",
                format!("session_id IN ({canonical})"),
            ),
            (
                "canonical_tool_results",
                format!("session_id IN ({canonical})"),
            ),
            (
                "canonical_compactions",
                format!("session_id IN ({canonical})"),
            ),
            (
                "canonical_context_epochs",
                format!("session_id IN ({canonical})"),
            ),
            (
                "canonical_session_relations",
                format!("source_session_id IN ({canonical})"),
            ),
            (
                "canonical_conversation_groups",
                format!("root_session_id IN ({canonical}) AND active"),
            ),
        ] {
            let n: i64 = self.database.connection().query_row(
                &format!("SELECT count(*) FROM {table} WHERE {predicate}"),
                [],
                |r| r.get(0),
            )?;
            out[table] = json!(n);
        }
        out["db"] = json!(self.path);
        out["schema_version"] = out["schema"].clone();
        out["imported_files"] = out["sources"].clone();
        out["messages"] = out["canonical_messages"].clone();
        if let Ok(errors) = self.database.connection().query_row(
            "SELECT value FROM meta WHERE key='projection_rebuild_errors'",
            [],
            |r| r.get::<_, String>(0),
        ) {
            out["projection_rebuild_errors"] = json!(errors);
        }
        for (name, query) in [
            (
                "missing_sources",
                format!("SELECT count(*) FROM sources WHERE missing AND id IN ({sources})"),
            ),
            (
                "pending_parser_upgrades",
                format!(
                    "SELECT count(*) FROM sources WHERE parser_version<>{} AND id IN ({sources})",
                    decoder_revision
                ),
            ),
            (
                "subagent_sessions",
                format!("SELECT count(*) FROM sessions WHERE subagent AND id IN ({sessions})"),
            ),
            (
                "projects",
                format!("SELECT count(*) FROM projects WHERE active AND profile_id IN ({ids})"),
            ),
            (
                "project_enriched_sessions",
                format!(
                    "SELECT count(DISTINCT session_id) \
                     FROM canonical_session_projects \
                     WHERE active AND session_id IN ({sessions})"
                ),
            ),
            (
                "token_events",
                format!("SELECT count(*) FROM token_events WHERE store_id IN ({ids})"),
            ),
            (
                "usage_events",
                format!("SELECT count(*) FROM usage_events WHERE profile_id IN ({ids})"),
            ),
        ] {
            out[name] = json!(
                self.database
                    .connection()
                    .query_row(&query, [], |r| r.get::<_, i64>(0))?
            );
        }
        for (prefix, table, predicate) in [
            ("message", "messages", format!("session_id IN ({sessions})")),
            ("token", "token_events", format!("store_id IN ({ids})")),
        ] {
            let (raw_min, raw_max, utc_min, utc_max) = self.database.connection().query_row(
                &format!(
                    "SELECT min(ts),max(ts),cast(min(timestamp_utc) AS VARCHAR),\
                     cast(max(timestamp_utc) AS VARCHAR) \
                     FROM {table} \
                     WHERE {predicate}"
                ),
                [],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )?;
            out[format!("{prefix}_min_ts")] = json!(raw_min);
            out[format!("{prefix}_max_ts")] = json!(raw_max);
            out[format!("{prefix}_min_timestamp_utc")] = json!(utc_min);
            out[format!("{prefix}_max_timestamp_utc")] = json!(utc_max);
        }
        Ok(out)
    }
}
