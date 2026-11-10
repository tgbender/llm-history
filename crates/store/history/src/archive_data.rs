//! Physical archive encoding, batch publication and bounded reconstruction.
//!
//! Records point to whole-document identities for stable occurrence IDs. Body
//! identities hash only extracted content, independently of those identities.
use anyhow::{Context, Result, ensure};
use duckdb::{Connection, Rows, params};
use llm_history_types::ingestion::ContentLeaf;
use std::collections::{HashMap, HashSet};

const HOLE: &[u8] = b"null";
const MAX_RECORD: usize = 64 * 1024 * 1024;
const RECENT_DOCUMENTS: usize = 65_536;

type ContentId = [u8; 32];

struct Document {
    hash: String,
    size: usize,
    checksum: u64,
    compressed: Vec<u8>,
    parts: Vec<(usize, String, ContentId)>,
}

#[derive(Default)]
pub(crate) struct ArchiveBatch {
    seen: HashSet<String>,
    recent: HashSet<String>,
    documents: Vec<Document>,
    bodies: HashMap<ContentId, (usize, Vec<u8>)>,
}

impl ArchiveBatch {
    pub(crate) fn prepare(
        &mut self,
        bytes: &[u8],
        leaves: Vec<ContentLeaf>,
    ) -> Result<(String, usize)> {
        let hash = blake3::hash(bytes).to_hex().to_string();
        if self.recent.contains(&hash) || !self.seen.insert(hash.clone()) {
            return Ok((hash, 0));
        }
        let mut layout = Vec::new();
        let mut parts = Vec::new();
        let mut stored = 0;
        let mut cursor = 0;
        for leaf in leaves {
            layout.extend_from_slice(&bytes[cursor..leaf.token.start]);
            let id = *blake3::hash(leaf.body.as_bytes()).as_bytes();
            if let std::collections::hash_map::Entry::Vacant(entry) = self.bodies.entry(id) {
                let body = zstd::bulk::compress(leaf.body.as_bytes(), 3)?;
                stored += body.len();
                entry.insert((leaf.body.len(), body));
            }
            stored += leaf.native_path.len() + 40;
            parts.push((layout.len(), leaf.native_path, id));
            layout.extend_from_slice(HOLE);
            cursor = leaf.token.end;
        }
        let document = if parts.is_empty() {
            bytes
        } else {
            layout.extend_from_slice(&bytes[cursor..]);
            &layout
        };
        let compressed = zstd::bulk::compress(document, 3)?;
        stored += compressed.len();
        self.documents.push(Document {
            hash: hash.clone(),
            size: bytes.len(),
            checksum: xxhash_rust::xxh64::xxh64(bytes, 0),
            compressed,
            parts,
        });
        Ok((hash, stored))
    }

    /// Called inside the same transaction as occurrences and the checkpoint.
    pub(crate) fn publish(&mut self, con: &Connection) -> Result<()> {
        if self.documents.is_empty() {
            return Ok(());
        }
        for table in ["native_documents", "content_parts", "content_objects"] {
            super::batch::stage(con, &format!("stage_{table}"), table)?;
        }
        let has_parts = !self.bodies.is_empty();
        {
            let mut documents =
                con.appender_to_catalog_and_db("stage_native_documents", "temp", "main")?;
            let mut parts =
                con.appender_to_catalog_and_db("stage_content_parts", "temp", "main")?;
            let mut bodies =
                con.appender_to_catalog_and_db("stage_content_objects", "temp", "main")?;
            for document in self.documents.drain(..) {
                for (offset, path, id) in document.parts {
                    parts.append_row(params![document.hash, offset as u64, path, id.as_slice()])?;
                }
                documents.append_row(params![
                    document.hash,
                    document.size as u64,
                    document.checksum,
                    document.compressed
                ])?;
            }
            for (id, (size, data)) in self.bodies.drain() {
                bodies.append_row(params![id.as_slice(), size as u64, data])?;
            }
            documents.flush()?;
            if has_parts {
                parts.flush()?;
                bodies.flush()?;
            }
        }
        con.execute_batch(include_str!("archive_publish.sql"))?;
        // Every staged hash now exists, including documents already stored in
        // an older representation. Reuse a bounded set across flushes.
        if self.recent.len().saturating_add(self.seen.len()) > RECENT_DOCUMENTS {
            self.recent.clear();
        }
        self.recent.extend(self.seen.drain().take(RECENT_DOCUMENTS));
        Ok(())
    }
}

enum Selection<'a> {
    Generations(&'a [i64]),
    Range {
        generation: i64,
        first: u64,
        last: u64,
    },
    Documents,
}

pub(crate) struct ArchivedRecord {
    pub generation: i64,
    pub raw_id: Option<i64>,
    pub ordinal: u64,
    pub start: u64,
    pub end: u64,
    pub hash: String,
    pub checksum: u64,
}

type Position = (i64, u64, String);

struct Part {
    position: Position,
    offset: usize,
    size: usize,
    data: Vec<u8>,
}

fn next_part(rows: &mut Rows<'_>) -> Result<Option<Part>> {
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(Part {
        position: (row.get(0)?, row.get(1)?, row.get(2)?),
        offset: row.get(3)?,
        size: row.get(4).context("missing content object")?,
        data: row.get(5)?,
    }))
}

/// Read documents and their content in two ordered, set-based cursors. There
/// is one lookahead part; the Rust reader assembles at most one record at a
/// time. A large replay does not repeatedly scan the content tables per page.
fn visit(
    con: &Connection,
    selection: Selection<'_>,
    mut visitor: impl FnMut(ArchivedRecord, Vec<u8>) -> Result<()>,
) -> Result<()> {
    let (selected, args) = match selection {
        Selection::Generations(ids) => (
            "SELECT generation_id,ordinal,start_byte,end_byte,payload_hash AS hash,raw_id
             FROM native_observations WHERE generation_id IN (SELECT unnest(?::BIGINT[]))",
            vec![super::embedding_batch::id_list(ids)],
        ),
        Selection::Range {
            generation,
            first,
            last,
        } => (
            "SELECT generation_id,ordinal,start_byte,end_byte,payload_hash AS hash,raw_id
             FROM native_observations WHERE generation_id=? AND ordinal BETWEEN ? AND ?",
            vec![
                duckdb::types::Value::BigInt(generation),
                duckdb::types::Value::UBigInt(first),
                duckdb::types::Value::UBigInt(last),
            ],
        ),
        Selection::Documents => (
            "SELECT 0 AS generation_id,0 AS ordinal,0 AS start_byte,0 AS end_byte,hash,NULL::BIGINT raw_id FROM native_documents",
            Vec::new(),
        ),
    };
    let mut statement = con.prepare(&format!(
        "WITH s AS ({selected})
         SELECT s.generation_id,s.ordinal,s.start_byte,s.end_byte,s.hash,p.raw_size,p.xxh64,p.data,s.raw_id
         FROM s LEFT JOIN native_documents p USING (hash) ORDER BY s.generation_id,s.ordinal,s.hash",
    ))?;
    let mut parts_statement = con.prepare(&format!(
        "WITH s AS ({selected})
         SELECT s.generation_id,s.ordinal,s.hash,c.layout_offset,o.byte_length,o.data
         FROM s JOIN content_parts c ON c.document_hash=s.hash
         LEFT JOIN content_objects o ON o.id=c.content_id
         ORDER BY s.generation_id,s.ordinal,s.hash,c.layout_offset",
    ))?;
    let mut rows = statement.query(duckdb::params_from_iter(&args))?;
    let mut part_rows = parts_statement.query(duckdb::params_from_iter(&args))?;
    let mut next = next_part(&mut part_rows)?;
    while let Some(row) = rows.next()? {
        let position: Position = (row.get(0)?, row.get(1)?, row.get(4)?);
        let size: usize = row.get(5).context("missing native document")?;
        ensure!(size <= MAX_RECORD, "archived record exceeds capture limit");
        let compressed: Vec<u8> = row.get(7)?;
        let layout = zstd::bulk::decompress(&compressed, size)?;
        let raw = if next.as_ref().is_some_and(|p| p.position == position) {
            let mut raw = Vec::with_capacity(size);
            let mut cursor = 0;
            while next.as_ref().is_some_and(|p| p.position == position) {
                let part = next.take().unwrap();
                let end = part
                    .offset
                    .checked_add(HOLE.len())
                    .context("invalid content offset")?;
                ensure!(
                    part.offset >= cursor && layout.get(part.offset..end) == Some(HOLE),
                    "invalid content offset"
                );
                raw.extend_from_slice(&layout[cursor..part.offset]);
                ensure!(
                    part.size <= size.saturating_sub(raw.len()),
                    "content exceeds record size"
                );
                let data = zstd::bulk::decompress(&part.data, part.size)?;
                ensure!(data.len() == part.size, "content size mismatch");
                llm_history_core::json_string::append_token(&mut raw, std::str::from_utf8(&data)?);
                ensure!(raw.len() <= size, "content exceeds record size");
                cursor = end;
                next = next_part(&mut part_rows)?;
            }
            raw.extend_from_slice(&layout[cursor..]);
            raw
        } else {
            ensure!(
                next.as_ref().is_none_or(|p| p.position > position),
                "out-of-order content reference"
            );
            layout
        };
        ensure!(raw.len() == size, "archived record size mismatch");
        visitor(
            ArchivedRecord {
                generation: position.0,
                raw_id: row.get(8)?,
                ordinal: position.1,
                start: row.get(2)?,
                end: row.get(3)?,
                hash: position.2,
                checksum: row.get(6)?,
            },
            raw,
        )?;
    }
    ensure!(next.is_none(), "unmatched content reference");
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) struct Generation {
    pub id: i64,
    pub lines: u64,
    pub bytes: u64,
}

const READ_ROWS: u64 = 20_000;
const READ_BYTES: u64 = 64 * 1024 * 1024;
const READ_GENERATIONS: usize = 64;

/// Bounded by decoded bytes and observations; an oversized generation gets
/// ordered ranges. Byte limits permit a single record to cross the boundary.
pub(crate) fn generation_groups<T>(
    mut rows: &[T],
    generation: impl Fn(&T) -> Generation,
) -> impl Iterator<Item = &[T]> {
    std::iter::from_fn(move || {
        if rows.is_empty() {
            return None;
        }
        let mut lines = 0u64;
        let mut bytes = 0u64;
        let mut count = 0;
        for row in rows.iter().take(READ_GENERATIONS) {
            let g = generation(row);
            if count > 0
                && (lines.saturating_add(g.lines) > READ_ROWS
                    || bytes.saturating_add(g.bytes) > READ_BYTES)
            {
                break;
            }
            lines = lines.saturating_add(g.lines);
            bytes = bytes.saturating_add(g.bytes);
            count += 1;
        }
        let (group, rest) = rows.split_at(count);
        rows = rest;
        Some(group)
    })
}

pub(crate) fn visit_generations(
    con: &Connection,
    group: &[Generation],
    mut read: impl FnMut(ArchivedRecord, Vec<u8>) -> Result<()>,
) -> Result<()> {
    if let [generation] = group
        && (generation.lines > READ_ROWS || generation.bytes > READ_BYTES)
    {
        // Plan ranges from narrow observation columns once. Keep all observed
        // ordinals so malformed surplus rows still reach the caller's checks.
        let mut st = con.prepare(
            "SELECT min(ordinal),max(ordinal)
             FROM native_observations WHERE generation_id=?
             GROUP BY ordinal // ?,start_byte // ? ORDER BY min(ordinal)",
        )?;
        let ranges = st
            .query_map(params![generation.id, READ_ROWS, READ_BYTES], |r| {
                Ok((r.get::<_, u64>(0)?, r.get::<_, u64>(1)?))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let mut prior = None;
        for (first, last) in ranges {
            ensure!(
                prior.is_none_or(|n| n < first),
                "archived record ranges overlap"
            );
            visit(
                con,
                Selection::Range {
                    generation: generation.id,
                    first,
                    last,
                },
                &mut read,
            )?;
            prior = Some(last);
        }
        return Ok(());
    }
    let ids: Vec<_> = group.iter().map(|g| g.id).collect();
    if ids.is_empty() {
        return Ok(());
    }
    visit(con, Selection::Generations(&ids), read)
}

pub(crate) fn visit_documents(
    con: &Connection,
    read: impl FnMut(ArchivedRecord, Vec<u8>) -> Result<()>,
) -> Result<()> {
    visit(con, Selection::Documents, read)
}
