//! Read retained inputs and publish prepared replacement projections.
use super::*;
use llm_history_types::ingestion::{ProjectionCheckpoint, ReplaySource, RetainedRecord};

impl Store {
    pub fn replay_sources(&self, profile: &Profile, ids: &[i64]) -> Result<Vec<ReplaySource>> {
        let mut st = self.database.connection().prepare(
            "SELECT s.id,s.source_key,s.uri,s.committed,s.ordinal,s.prefix_hash,s.parser_state,
                    s.generation,s.missing,s.observed_size,s.observed_mtime_ns,s.head_xxh64,
                    s.boundary_xxh64,s.parser_version,g.id
             FROM sources s LEFT JOIN generations g ON g.source_id=s.id AND g.number=s.generation
             WHERE s.profile_id=? AND (? OR s.id IN (SELECT unnest(?::BIGINT[]))) ORDER BY s.id",
        )?;
        Ok(st
            .query_map(
                params![profile.id, ids.is_empty(), embedding_batch::id_list(ids)],
                |r| {
                    Ok(ReplaySource {
                        profile: profile.clone(),
                        checkpoint: crate::publication::checkpoint(r)?,
                        generation_id: r.get(14)?,
                    })
                },
            )?
            .collect::<duckdb::Result<_>>()?)
    }

    pub fn retained_records(
        &self,
        sources: &[&ReplaySource],
        mut read: impl FnMut(usize, RetainedRecord) -> Result<()>,
    ) -> Result<()> {
        use std::collections::BTreeMap;
        let generations: Vec<_> = sources
            .iter()
            .map(|source| archive_data::Generation {
                id: source.generation_id,
                lines: source.checkpoint.ordinal,
                bytes: source.checkpoint.committed,
            })
            .collect();
        let mut positions: BTreeMap<_, _> = generations
            .iter()
            .enumerate()
            .map(|(i, g)| (g.id, (i, 0)))
            .collect();
        anyhow::ensure!(
            positions.len() == sources.len(),
            "duplicate replay generation"
        );
        for group in archive_data::generation_groups(&generations, |g| *g) {
            archive_data::visit_generations(self.database.connection(), group, |record, bytes| {
                let (index, ordinal) = positions
                    .get_mut(&record.generation)
                    .context("unexpected retained generation")?;
                anyhow::ensure!(
                    record.ordinal == *ordinal + 1,
                    "retained record sequence is discontinuous"
                );
                *ordinal = record.ordinal;
                read(
                    *index,
                    RetainedRecord {
                        raw_id: record.raw_id,
                        ordinal: record.ordinal,
                        hash: record.hash,
                        bytes,
                    },
                )
            })?;
        }
        for (index, ordinal) in positions.into_values() {
            anyhow::ensure!(
                ordinal == sources[index].checkpoint.ordinal,
                "retained archive ended before its checkpoint"
            );
        }
        Ok(())
    }

    pub fn publish_projections<I>(
        &self,
        prepared: Vec<(&ReplaySource, &ProjectionCheckpoint, I)>,
    ) -> Result<()>
    where
        I: Iterator<Item = Result<PreparedRecord>>,
    {
        if prepared.is_empty() {
            return Ok(());
        }
        let tx = self.database.begin()?;
        let con = tx.connection();
        con.execute_batch(
            "CREATE TEMP TABLE replay_plan (
                source_id BIGINT,profile_id VARCHAR,parser_state VARCHAR,parser_version INTEGER, slot BIGINT
            )",
        )?;
        let mut ids_seen = std::collections::BTreeSet::new();
        {
            let mut app = con.appender_to_catalog_and_db("replay_plan", "temp", "main")?;
            for (slot, (source, projection, _)) in prepared.iter().enumerate() {
                anyhow::ensure!(
                    ids_seen.insert(source.checkpoint.id),
                    "duplicate replay source"
                );
                app.append_row(params![
                    source.checkpoint.id,
                    source.profile.id,
                    projection.continuation,
                    projection.decoder_revision,
                    slot as i64
                ])?;
            }
            app.flush()?;
        }
        {
            let mut st = con.prepare(
                "SELECT s.id,s.source_key,s.uri,s.committed,s.ordinal,s.prefix_hash,s.parser_state,
                        s.generation,s.missing,s.observed_size,s.observed_mtime_ns,s.head_xxh64,
                        s.boundary_xxh64,s.parser_version,g.id
                 FROM temp.replay_plan p LEFT JOIN sources s ON s.id=p.source_id AND s.profile_id=p.profile_id
                 LEFT JOIN generations g ON g.source_id=s.id AND g.number=s.generation ORDER BY p.slot",
            )?;
            let current = st.query_map([], |r| {
                Ok((
                    r.get::<_, Option<i64>>(0)?
                        .map(|_| crate::publication::checkpoint(r))
                        .transpose()?,
                    r.get::<_, Option<i64>>(14)?,
                ))
            })?;
            for ((expected, _, _), row) in prepared.iter().zip(current) {
                let (checkpoint, generation) = row?;
                anyhow::ensure!(
                    checkpoint.as_ref() == Some(&expected.checkpoint)
                        && generation == Some(expected.generation_id),
                    "source checkpoint changed during preparation"
                );
            }
        }
        let ids = prepared
            .iter()
            .map(|(s, _, _)| s.checkpoint.id)
            .collect::<Vec<_>>();
        self.clear_sources(&ids)?;
        let mut batch = Batch::new(self, ids)?;
        let finished = prepared
            .iter()
            .map(|(s, p, _)| (s.checkpoint.id, &s.profile, &p.session))
            .collect::<Vec<_>>();
        for (source, _, records) in prepared {
            let mut ordinal = 0;
            for record in records {
                let record = record?;
                anyhow::ensure!(
                    record.ordinal == ordinal + 1,
                    "prepared replay sequence is discontinuous"
                );
                ordinal = record.ordinal;
                batch.write_record(
                    self,
                    &source.profile,
                    source.checkpoint.id,
                    (source.generation_id, None),
                    record,
                )?;
            }
            anyhow::ensure!(
                ordinal == source.checkpoint.ordinal,
                "prepared replay ended before its checkpoint"
            );
        }
        con.execute_batch(
            "UPDATE sources s SET parser_state=p.parser_state,parser_version=p.parser_version
             FROM temp.replay_plan p WHERE s.id=p.source_id;
             DROP TABLE temp.replay_plan;",
        )?;
        batch.flush(self)?;
        self.finish_sources(&finished)?;
        tx.commit()
    }
}
