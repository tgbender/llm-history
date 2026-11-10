//! Typed checkpoint queries and atomic publication of prepared transcript records.
use super::*;
use llm_history_types::ingestion::{
    CaptureKind, PreparedCapture, PreparedRecord, SourceCheckpoint, SourceStamp,
};
use std::collections::{BTreeSet, HashMap};

const CHECKPOINT: &str = "SELECT id,source_key,uri,committed,ordinal,prefix_hash,parser_state,generation,missing,observed_size,observed_mtime_ns,head_xxh64,boundary_xxh64,parser_version FROM sources";

pub(crate) fn checkpoint(r: &duckdb::Row<'_>) -> duckdb::Result<SourceCheckpoint> {
    Ok(SourceCheckpoint {
        id: r.get(0)?,
        key: r.get(1)?,
        uri: r.get(2)?,
        committed: r.get(3)?,
        ordinal: r.get(4)?,
        prefix_hash: r.get(5)?,
        continuation: r.get(6)?,
        generation: r.get(7)?,
        missing: r.get(8)?,
        size: r.get(9)?,
        mtime_ns: r.get(10)?,
        head: r.get(11)?,
        boundary: r.get(12)?,
        decoder_revision: r.get(13)?,
    })
}

impl Store {
    pub fn source_stamps(&self, profile: &Profile) -> Result<Vec<SourceStamp>> {
        let mut statement = self.database.connection().prepare(
            "SELECT source_key,uri,missing,observed_size,observed_mtime_ns,parser_version,prefix_hash \
             FROM sources WHERE profile_id=?",
        )?;
        Ok(statement
            .query_map([&profile.id], |row| {
                Ok(SourceStamp {
                    key: row.get(0)?,
                    uri: row.get(1)?,
                    missing: row.get(2)?,
                    size: row.get(3)?,
                    mtime_ns: row.get(4)?,
                    decoder_revision: row.get(5)?,
                    source_token: row.get(6)?,
                })
            })?
            .collect::<duckdb::Result<_>>()?)
    }

    pub fn source_checkpoints(&self, profile: &Profile) -> Result<Vec<SourceCheckpoint>> {
        let mut st = self
            .database
            .connection()
            .prepare(&format!("{CHECKPOINT} WHERE profile_id=? ORDER BY id"))?;
        Ok(st
            .query_map([&profile.id], checkpoint)?
            .collect::<duckdb::Result<_>>()?)
    }

    pub fn usage_keys(&self, sources: &[i64]) -> Result<HashMap<i64, BTreeSet<String>>> {
        let mut output = HashMap::<_, BTreeSet<_>>::new();
        if sources.is_empty() {
            return Ok(output);
        }
        let mut st = self.database.connection().prepare(
            "SELECT DISTINCT source_id,usage_key FROM events
             WHERE source_id IN (SELECT unnest(?::BIGINT[])) AND active AND usage_key<>''",
        )?;
        for row in st.query_map([embedding_batch::id_list(sources)], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })? {
            let (id, key) = row?;
            output.entry(id).or_default().insert(key);
        }
        Ok(output)
    }

    pub fn refresh_source_locations(
        &self,
        profile: &Profile,
        changes: &[(SourceCheckpoint, PathBuf)],
    ) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let tx = self.database.begin()?;
        let con = tx.connection();
        let ids = changes.iter().map(|(old, _)| old.id).collect::<Vec<_>>();
        let observed: HashMap<_, _> = {
            let mut st = con.prepare(&format!(
                "{CHECKPOINT} WHERE profile_id=? AND id IN (SELECT unnest(?::BIGINT[]))"
            ))?;
            st.query_map(
                params![profile.id, embedding_batch::id_list(&ids)],
                checkpoint,
            )?
            .map(|row| row.map(|row| (row.id, row)))
            .collect::<duckdb::Result<_>>()?
        };
        anyhow::ensure!(
            changes
                .iter()
                .all(|(prior, _)| observed.get(&prior.id) == Some(prior)),
            "source checkpoint changed during preparation"
        );
        let paths = duckdb::types::Value::List(
            changes
                .iter()
                .map(|(_, p)| duckdb::types::Value::Text(p.to_string_lossy().into_owned()))
                .collect(),
        );
        con.execute(
            "UPDATE sources s SET uri=b.uri,missing=false FROM
             (SELECT unnest(?::BIGINT[]) id,unnest(?::VARCHAR[]) uri) b
             WHERE s.id=b.id AND s.profile_id=? AND (s.uri IS DISTINCT FROM b.uri OR s.missing)",
            params![embedding_batch::id_list(&ids), paths, profile.id],
        )?;
        tx.commit()
    }

    pub fn reconcile_inventory(&self, profile: &Profile, keys: &BTreeSet<String>) -> Result<usize> {
        let keys = duckdb::types::Value::List(
            keys.iter()
                .cloned()
                .map(duckdb::types::Value::Text)
                .collect(),
        );
        let con = self.database.connection();
        con.execute(
            "WITH observed AS (SELECT unnest(?::VARCHAR[]) source_key),
             status AS (
                SELECT s.id,o.source_key IS NULL missing FROM sources s
                LEFT JOIN observed o USING(source_key) WHERE s.profile_id=?
             )
             UPDATE sources s SET missing=b.missing FROM status b
             WHERE s.id=b.id AND s.missing IS DISTINCT FROM b.missing",
            params![keys, profile.id],
        )?;
        Ok(con.query_row(
            "SELECT count(*) FROM sources WHERE profile_id=? AND missing",
            [&profile.id],
            |r| r.get(0),
        )?)
    }

    pub fn reconcile_conversations(&self, force: bool) -> Result<()> {
        let con = self.database.connection();
        let profiles = {
            let mut st = con.prepare(
                "SELECT substr(key,length('conversation_graph_pending:')+1)
                 FROM meta WHERE starts_with(key,'conversation_graph_pending:')",
            )?;
            st.query_map([], |r| Ok(duckdb::types::Value::Text(r.get(0)?)))?
                .collect::<duckdb::Result<Vec<_>>>()?
        };
        if !force && profiles.is_empty() {
            return Ok(());
        }
        let tx = self.database.begin()?;
        con.execute(
            "CREATE TEMP TABLE graph_sessions AS SELECT id FROM canonical_sessions
             WHERE ? OR store_id IN (SELECT unnest(?::VARCHAR[]))",
            params![force, duckdb::types::Value::List(profiles)],
        )?;
        self.refresh_conversations()?;
        con.execute_batch(
            "DELETE FROM meta WHERE starts_with(key,'conversation_graph_pending:');
             DROP TABLE temp.graph_sessions;",
        )?;
        tx.commit()
    }

    /// Publish a bounded group of prepared inputs atomically. The only reads of
    /// native records are from the caller's prepared iterators.
    pub fn publish_captures<'a, I: Iterator<Item = Result<PreparedRecord>>>(
        &self,
        profile: &Profile,
        prepared: impl IntoIterator<Item = (&'a PreparedCapture, I)>,
    ) -> Result<Vec<(CaptureKind, usize)>> {
        let inputs: Vec<_> = prepared.into_iter().collect();
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let tx = self.database.begin()?;
        let con = tx.connection();
        let plans = capture_plans(con, profile, &inputs)?;
        let rebuilt: Vec<_> = plans
            .iter()
            .zip(&inputs)
            .filter(|(_, (work, _))| work.expected.is_some() && work.kind != CaptureKind::Append)
            .map(|(plan, _)| plan.source)
            .collect();
        self.clear_sources(&rebuilt)?;
        stage_captures(con, profile, &plans, &inputs)?;
        let mut batch = Batch::new(self, rebuilt)?;
        if inputs.iter().any(|(work, _)| {
            work.kind == CaptureKind::Append && work.end_ordinal > work.start_ordinal
        }) {
            con.execute_batch(include_str!("append_epoch_seeds.sql"))?;
        }
        let mut finished = Vec::new();
        let mut outcomes = Vec::with_capacity(inputs.len());
        for (plan, (work, records)) in plans.iter().zip(inputs) {
            let mut end = work.start;
            let mut ordinal = work.start_ordinal;
            for record in records {
                let record = record?;
                anyhow::ensure!(
                    record.ordinal == ordinal + 1,
                    "noncontiguous prepared record sequence"
                );
                let raw = record
                    .raw
                    .as_ref()
                    .context("prepared capture lacks native bytes")?;
                anyhow::ensure!(
                    raw.start == end
                        && raw.end.checked_sub(raw.start) == Some(raw.bytes.len() as u64),
                    "noncontiguous prepared capture"
                );
                end = raw.end;
                ordinal = record.ordinal;
                batch.write_record(
                    self,
                    profile,
                    plan.source,
                    (plan.generation, Some(plan.capture)),
                    record,
                )?;
            }
            anyhow::ensure!(
                end == work.end && ordinal == work.end_ordinal,
                "prepared capture ended before its checkpoint"
            );
            let count = (ordinal - work.start_ordinal) as usize;
            if work.kind != CaptureKind::Append || count > 0 {
                finished.push((plan.source, profile, &work.projection.session));
            }
            outcomes.push((work.kind, count));
        }
        batch.flush(self)?;
        con.execute_batch(include_str!("publication_finish.sql"))?;
        self.finish_sources(&finished)?;
        tx.commit()?;
        Ok(outcomes)
    }
}

struct CapturePlan {
    source: i64,
    generation: i64,
    old_generation: Option<i64>,
    capture: i64,
}

fn capture_plans<I>(
    con: &Connection,
    profile: &Profile,
    inputs: &[(&PreparedCapture, I)],
) -> Result<Vec<CapturePlan>> {
    let mut keys = BTreeSet::new();
    let mut expected_ids = BTreeSet::new();
    con.execute_batch(
        "CREATE OR REPLACE TEMP TABLE capture_plan (
            slot BIGINT, profile_id VARCHAR, source_key VARCHAR, expected_id BIGINT
        )",
    )?;
    {
        let mut app = con.appender_to_catalog_and_db("capture_plan", "temp", "main")?;
        for (slot, (work, _)) in inputs.iter().enumerate() {
            let expected = work.expected.as_ref().map(|s| s.id);
            anyhow::ensure!(
                keys.insert(&work.key) && expected.is_none_or(|id| expected_ids.insert(id)),
                "duplicate source in capture batch"
            );
            anyhow::ensure!(
                (work.kind == CaptureKind::New) == expected.is_none(),
                "capture kind does not match its checkpoint"
            );
            app.append_row(params![slot as i64, profile.id, work.key, expected])?;
        }
        app.flush()?;
    }
    let current = {
        let mut st = con.prepare(
            "SELECT s.id,s.source_key,s.uri,s.committed,s.ordinal,s.prefix_hash,s.parser_state,
                    s.generation,s.missing,s.observed_size,s.observed_mtime_ns,s.head_xxh64,
                    s.boundary_xxh64,s.parser_version,k.id,g.id,g.bytes
             FROM temp.capture_plan p
             LEFT JOIN sources s ON s.profile_id=p.profile_id AND s.id=p.expected_id
             LEFT JOIN sources k ON k.profile_id=p.profile_id AND k.source_key=p.source_key
             LEFT JOIN generations g ON g.source_id=s.id AND g.number=s.generation
             ORDER BY p.slot",
        )?;
        st.query_map([], |r| {
            Ok((
                r.get::<_, Option<i64>>(0)?
                    .map(|_| checkpoint(r))
                    .transpose()?,
                r.get::<_, Option<i64>>(14)?,
                r.get::<_, Option<i64>>(15)?,
                r.get::<_, Option<u64>>(16)?,
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?
    };
    con.execute_batch("DROP TABLE temp.capture_plan")?;
    let entity_count = inputs
        .iter()
        .map(|(w, _)| {
            usize::from(w.expected.is_none()) + usize::from(w.kind != CaptureKind::Append)
        })
        .sum();
    let mut entities = crate::batch::allocate(con, entity_count)?.into_iter();
    let captures = {
        let mut st = con.prepare("SELECT nextval('capture_ids') id FROM range(?) ORDER BY id")?;
        st.query_map([inputs.len() as i64], |r| r.get(0))?
            .collect::<duckdb::Result<Vec<_>>>()?
    };
    inputs
        .iter()
        .zip(current)
        .zip(captures)
        .map(|(((work, _), (source, key_owner, old, bytes)), capture)| {
            anyhow::ensure!(
                source.as_ref() == work.expected.as_ref()
                    && key_owner.is_none_or(|id| source.as_ref().is_some_and(|s| s.id == id)),
                "source checkpoint changed during preparation"
            );
            let source_id = match source.as_ref() {
                Some(row) => row.id,
                None => entities.next().context("source ID")?,
            };
            let generation = if work.kind == CaptureKind::Append {
                let prior = source.as_ref().context("append has no checkpoint")?;
                anyhow::ensure!(
                    bytes == Some(work.start)
                        && prior.committed == work.start
                        && prior.ordinal == work.start_ordinal,
                    "generation does not match the planned offset"
                );
                old.context("append has no generation")?
            } else {
                anyhow::ensure!(
                    work.start == 0 && work.start_ordinal == 0,
                    "replacement starts after the beginning"
                );
                entities.next().context("generation ID")?
            };
            Ok(CapturePlan {
                source: source_id,
                generation,
                old_generation: old,
                capture,
            })
        })
        .collect()
}

fn stage_captures<I>(
    con: &Connection,
    profile: &Profile,
    plans: &[CapturePlan],
    inputs: &[(&PreparedCapture, I)],
) -> Result<()> {
    con.execute_batch(
        "CREATE OR REPLACE TEMP TABLE capture_finish (
            source_id BIGINT, generation_id BIGINT, old_generation_id BIGINT, capture_id BIGINT,
            profile_id VARCHAR, source_key VARCHAR, uri VARCHAR, encoding VARCHAR, kind VARCHAR,
            generation BIGINT, physical_cutoff UBIGINT, start_ordinal UBIGINT, start_byte UBIGINT,
            end_ordinal UBIGINT, end_byte UBIGINT, complete BOOLEAN, mtime_ns BIGINT,
            head_xxh64 UBIGINT, boundary_xxh64 UBIGINT, parser_state VARCHAR, parser_version INTEGER,
            source_token VARCHAR
        )",
    )?;
    {
        let mut app = con.appender_to_catalog_and_db("capture_finish", "temp", "main")?;
        for (plan, (work, _)) in plans.iter().zip(inputs) {
            let generation = work.expected.as_ref().map_or(1, |prior| {
                prior.generation + i64::from(work.kind != CaptureKind::Append)
            });
            app.append_row(params![
                plan.source,
                plan.generation,
                plan.old_generation,
                plan.capture,
                profile.id,
                work.key,
                work.path.to_string_lossy(),
                work.encoding.label(),
                work.kind.label(),
                generation,
                work.physical_cutoff,
                work.start_ordinal,
                work.start,
                work.end_ordinal,
                work.end,
                work.complete,
                work.mtime_ns,
                work.windows.0,
                work.windows.1,
                work.projection.continuation,
                work.projection.decoder_revision,
                work.source_token
            ])?;
        }
        app.flush()?;
    }
    con.execute_batch(
        "INSERT INTO generations
         SELECT generation_id,source_id,generation,kind,0,0,'',false
         FROM temp.capture_finish WHERE kind<>'append'",
    )?;
    Ok(())
}
