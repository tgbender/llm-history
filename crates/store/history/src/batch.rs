//! Buffered projection writes. Facts from any number of sources go through DuckDB's
//! appender into one staging table, then a few set-based statements per batch.
use super::*;
use crate::session_batch::SessionBatch;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Staged facts carry the fact-table columns that come from parsing; sequence,
/// epoch, generation and raw record columns are derived in `canonical_batch.sql`.
const STAGE_EVENTS: &str = "CREATE TEMP TABLE IF NOT EXISTS stage_events(id BIGINT,\
                            source_id BIGINT,session_id BIGINT,ordinal UBIGINT,\
                            fact_index BIGINT,payload_hash VARCHAR,kind VARCHAR,ts VARCHAR,\
                            role VARCHAR,phase VARCHAR,text VARCHAR,text_hash VARCHAR,\
                            model VARCHAR,tool VARCHAR,call_id VARCHAR,input_tokens BIGINT,\
                            output_tokens BIGINT,cache_read BIGINT,cache_write BIGINT,\
                            excluded BOOLEAN,usage_key VARCHAR,effort VARCHAR,\
                            thinking_tokens BIGINT,usage_eligible BOOLEAN,\
                            timestamp_utc VARCHAR,timestamp_offset_minutes INTEGER,\
                            timestamp_timezone VARCHAR,timestamp_basis VARCHAR,\
                            turn_id VARCHAR,native_event_type VARCHAR,\
                            native_payload_type VARCHAR,native_item_id VARCHAR,\
                            native_parent_id VARCHAR,metadata_json VARCHAR,\
                            native_turn_id VARCHAR,arguments_json VARCHAR,\
                            agent_protocol BOOLEAN,generation_id BIGINT,raw_id BIGINT)";
/// Restages stored facts, for example when a source changes session owner.
pub(crate) const RESTAGE_EVENTS: &str = "INSERT INTO temp.stage_events \
                                         SELECT id,source_id,session_id,ordinal,fact_index,payload_hash,\
                                         kind,ts,role,phase,text,text_hash,model,tool,call_id,\
                                         input_tokens,output_tokens,cache_read,cache_write,excluded,\
                                         usage_key,effort,thinking_tokens,usage_eligible,\
                                         CAST(timestamp_utc AS VARCHAR),timestamp_offset_minutes,\
                                         timestamp_timezone,timestamp_basis,turn_id,native_event_type,\
                                         native_payload_type,native_item_id,native_parent_id,\
                                         metadata_json,native_turn_id,arguments_json,agent_protocol,generation_id,raw_record_id \
                                         FROM events";

pub(crate) fn create_stage(con: &Connection) -> Result<()> {
    con.execute_batch(STAGE_EVENTS)?;
    stage(con, "stage_lexical", "lexical_documents")
}

#[derive(Clone, Copy)]
enum NativeRow {
    Capture(usize),
    Retained(Option<i64>),
}

struct EventRow {
    generation: i64,
    native: NativeRow,
    source: i64,
    session: usize,
    ordinal: u64,
    hash: String,
    fact: Fact,
}

struct Observation {
    generation: i64,
    capture: Option<i64>,
    ordinal: u64,
    start: u64,
    end: u64,
    document: String,
    json_valid: bool,
}

pub(crate) struct Batch {
    reuse: BTreeSet<i64>,
    sessions: SessionBatch,
    archive: archive_data::ArchiveBatch,
    observations: Vec<Observation>,
    events: Vec<EventRow>,
    bytes: usize,
}

impl Batch {
    pub(crate) fn new(store: &Store, rebuilt: impl IntoIterator<Item = i64>) -> Result<Self> {
        store.start_projection()?;
        Ok(Self {
            reuse: rebuilt.into_iter().collect(),
            sessions: SessionBatch::default(),
            archive: archive_data::ArchiveBatch::default(),
            observations: Vec::new(),
            events: Vec::new(),
            bytes: 0,
        })
    }

    // Probe only exact staged fact keys for replacements and replay. Ordinary
    // appends need no lookup; they start beyond the committed ordinal.
    fn existing_ids(&self, con: &Connection) -> Result<Vec<Option<i64>>> {
        let mut ids = vec![None; self.events.len()];
        if self.reuse.is_empty() || !self.events.iter().any(|e| self.reuse.contains(&e.source)) {
            return Ok(ids);
        }
        con.execute_batch(
            "CREATE OR REPLACE TEMP TABLE stage_fact_keys (
               slot BIGINT, source_id BIGINT, ordinal UBIGINT, fact_index BIGINT, payload_hash VARCHAR
             )",
        )?;
        {
            let mut app = con.appender_to_catalog_and_db("stage_fact_keys", "temp", "main")?;
            for (slot, e) in self
                .events
                .iter()
                .enumerate()
                .filter(|(_, e)| self.reuse.contains(&e.source))
            {
                app.append_row(params![
                    slot as i64,
                    e.source,
                    e.ordinal,
                    e.fact.index as i64,
                    e.hash
                ])?;
            }
            app.flush()?;
        }
        {
            let mut st = con.prepare(
                "SELECT k.slot,max(e.id) FROM temp.stage_fact_keys k JOIN events e
                 USING (source_id,ordinal,fact_index,payload_hash) GROUP BY k.slot",
            )?;
            for row in st.query_map([], |r| Ok((r.get::<_, usize>(0)?, r.get::<_, i64>(1)?)))? {
                let (slot, id) = row?;
                ids[slot] = Some(id);
            }
        }
        con.execute_batch("DROP TABLE temp.stage_fact_keys")?;
        Ok(ids)
    }

    pub(crate) fn write_record(
        &mut self,
        store: &Store,
        profile: &Profile,
        source: i64,
        (generation, capture): (i64, Option<i64>),
        record: PreparedRecord,
    ) -> Result<()> {
        let (hash, native) = match (record.raw, capture) {
            (Some(raw), Some(capture)) => {
                let (hash, stored) = self.archive.prepare(&raw.bytes, raw.leaves)?;
                self.bytes = self.bytes.saturating_add(stored.max(raw.bytes.len()));
                let slot = self.observations.len();
                self.observations.push(Observation {
                    generation,
                    capture: Some(capture),
                    ordinal: record.ordinal,
                    start: raw.start,
                    end: raw.end,
                    document: hash.clone(),
                    json_valid: raw.json_valid,
                });
                (hash, NativeRow::Capture(slot))
            }
            (None, None) => (
                record.document_hash,
                NativeRow::Retained(record.retained_raw_id),
            ),
            _ => bail!("prepared record does not match the publication kind"),
        };
        if let Some(session) = record.session {
            let (slot, bytes) = self.sessions.push(profile, session);
            self.bytes = self.bytes.saturating_add(bytes);
            for fact in record.facts {
                self.bytes = self.bytes.saturating_add(
                    fact.text.len()
                        + fact.canonical.metadata.len()
                        + fact
                            .canonical
                            .arguments_json
                            .as_ref()
                            .map_or(0, String::len),
                );
                self.events.push(EventRow {
                    generation,
                    native,
                    source,
                    session: slot,
                    ordinal: record.ordinal,
                    hash: hash.clone(),
                    fact,
                });
            }
        }
        self.maybe_flush(store)
    }
    pub(crate) fn maybe_flush(&mut self, store: &Store) -> Result<()> {
        if self
            .observations
            .len()
            .max(self.events.len())
            .max(self.sessions.len())
            >= store.options.batch_records
            || self.bytes >= store.options.batch_bytes
        {
            self.flush(store)?;
        }
        Ok(())
    }
    pub(crate) fn flush(&mut self, store: &Store) -> Result<()> {
        let con = &store.database.connection();
        let session_ids = self.sessions.publish(store)?;
        let existing = self.existing_ids(con)?;
        let missing = existing.iter().filter(|id| id.is_none()).count();
        let mut ids = allocate(con, self.observations.len() + missing)?.into_iter();
        self.archive.publish(con)?;
        let mut raw_ids = Vec::with_capacity(self.observations.len());
        if !self.observations.is_empty() {
            let mut app = con.appender("native_observations")?;
            for o in self.observations.drain(..) {
                let id = ids.next().context("record ID")?;
                raw_ids.push(id);
                app.append_row(params![
                    o.generation,
                    o.ordinal,
                    o.start,
                    o.end,
                    o.document,
                    id,
                    o.json_valid,
                    o.capture
                ])?;
            }
            app.flush()?;
        }
        if !self.events.is_empty() {
            create_stage(con)?;
            let mut provider = crate::provider::ProviderBatch::new();
            let mut events = con.appender_to_catalog_and_db("stage_events", "temp", "main")?;
            let mut lexical = con.appender_to_catalog_and_db("stage_lexical", "temp", "main")?;
            for (e, existing) in self.events.drain(..).zip(existing) {
                let id = match existing {
                    Some(id) => id,
                    None => ids.next().context("event ID")?,
                };
                let f = e.fact;
                // Provider time and native identity describe the physical record.
                let record = f.index == 0;
                let (d, n) = (&f.detail, &f.canonical);
                // Python's typed tool-result field uses SHA-256. This replaces
                // its unused BLAKE3 text hash; each output is hashed once here.
                let text_hash = if f.kind == "tool_result" {
                    hex::encode(Sha256::digest(f.text.as_bytes()))
                } else {
                    blake3::hash(f.text.as_bytes()).to_hex().to_string()
                };
                events.append_row(params![
                    id,
                    e.source,
                    session_ids[e.session],
                    e.ordinal,
                    f.index as i64,
                    e.hash,
                    f.kind,
                    f.timestamp,
                    f.role,
                    f.phase,
                    f.text,
                    text_hash,
                    f.model,
                    f.tool,
                    f.call_id,
                    f.usage.input,
                    f.usage.output,
                    f.usage.cache_read,
                    f.usage.cache_write,
                    f.excluded,
                    f.usage_key,
                    f.effort,
                    f.usage.thinking,
                    f.usage_eligible,
                    record.then_some(d.time.utc.as_deref()).flatten(),
                    record.then_some(d.time.offset_minutes).flatten(),
                    record.then_some(d.time.timezone.as_str()),
                    record.then_some(d.time.basis.as_str()),
                    record.then_some(d.turn_id.as_str()),
                    record.then_some(n.native_type.as_str()),
                    record.then_some(n.payload_type.as_str()),
                    record.then_some(n.item_id.as_str()),
                    record.then_some(n.parent_id.as_str()),
                    record.then_some(n.metadata.as_str()),
                    record.then_some(n.turn_id.as_str()),
                    record.then_some(n.arguments_json.as_deref()).flatten(),
                    record && n.agent_protocol,
                    e.generation,
                    match e.native {
                        NativeRow::Capture(i) => Some(raw_ids[i]),
                        NativeRow::Retained(id) => id,
                    }
                ])?;
                if f.kind == "message"
                    && !f.text.is_empty()
                    && !f.excluded
                    && (f.role == "user" || (f.role == "assistant" && f.phase == "final_answer"))
                {
                    let searched = llm_history_core::fact_search_text(&f);
                    lexical.append_row(params![id, e.source, searched, code_aliases(searched)])?;
                }
                if record {
                    provider.push(id, f.detail);
                }
            }
            drop((events, lexical));
            store.project_events()?;
            provider.flush(store)?;
            con.execute_batch("DELETE FROM temp.stage_events")?;
        }
        self.bytes = 0;
        Ok(())
    }
}

pub(crate) fn stage(con: &Connection, name: &str, like: &str) -> Result<()> {
    con.execute_batch(&format!(
        "create temp table if not exists {name} as select * from {like} limit 0"
    ))?;
    Ok(())
}

/// IDs come back ascending, so rows keep their insertion order.
pub(crate) fn allocate(con: &Connection, n: usize) -> Result<Vec<i64>> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut st = con.prepare("select nextval('entity_ids') id from range(?) order by id")?;
    Ok(st
        .query_map([n as i64], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?)
}
