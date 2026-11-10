use super::*;
use std::io::Read;

struct SourceGeneration {
    source: RawSource,
    generation_id: i64,
    sqlite_rows: bool,
}
impl SourceGeneration {
    fn archive(&self) -> archive_data::Generation {
        archive_data::Generation {
            id: self.generation_id,
            lines: self.source.lines,
            bytes: self.source.bytes,
        }
    }
}

struct VerifyState {
    live: Option<Box<dyn Read>>,
    end: u64,
    lines: u64,
}

fn verify_group(
    con: &Connection,
    group: &[SourceGeneration],
    against_live: bool,
    checked: &mut std::collections::HashSet<String>,
) -> Result<Vec<Result<()>>> {
    let mut states = group
        .iter()
        .map(|row| {
            let live: Option<Box<dyn Read>> = if against_live {
                anyhow::ensure!(
                    !row.sqlite_rows,
                    "live byte comparison is unavailable for retained SQLite rows"
                );
                let path = &row.source.path;
                let file = File::open(path)
                    .with_context(|| format!("live source is missing: {}", path.display()))?;
                Some(if llm_history_core::is_compressed(path) {
                    Box::new(zstd::stream::read::Decoder::new(file)?)
                } else {
                    Box::new(file)
                })
            } else {
                None
            };
            Ok(VerifyState {
                live,
                end: 0,
                lines: 0,
            })
        })
        .collect::<Vec<Result<_>>>();
    let positions: std::collections::HashMap<_, _> = group
        .iter()
        .enumerate()
        .map(|(i, row)| (row.generation_id, i))
        .collect();
    let selected: Vec<_> = group
        .iter()
        .zip(&states)
        .filter(|(_, s)| s.is_ok())
        .map(|(row, _)| row.archive())
        .collect();
    archive_data::visit_generations(con, &selected, |record, raw| {
        let entry = &mut states[*positions
            .get(&record.generation)
            .context("unexpected archived generation")?];
        if let Ok(state) = entry {
            let result = (|| {
                anyhow::ensure!(
                    record.ordinal == state.lines + 1
                        && record.start == state.end
                        && record.end.checked_sub(record.start) == Some(raw.len() as u64),
                    "archived records do not tile the source generation"
                );
                if state.live.is_some() || !checked.contains(&record.hash) {
                    anyhow::ensure!(
                        xxhash_rust::xxh64::xxh64(&raw, 0) == record.checksum,
                        "raw payload integrity error"
                    );
                    checked.insert(record.hash);
                    if let Some(file) = &mut state.live {
                        let mut observed = vec![0; raw.len()];
                        file.read_exact(&mut observed)?;
                        anyhow::ensure!(
                            observed == raw,
                            "live source differs from the archived prefix"
                        );
                    }
                }
                state.lines += 1;
                state.end = record.end;
                Ok(())
            })();
            if let Err(error) = result {
                *entry = Err(error);
            }
        }
        Ok(())
    })?;
    Ok(group
        .iter()
        .zip(states)
        .map(|(row, state)| {
            let s = state?;
            anyhow::ensure!(
                s.lines == row.source.lines && s.end == row.source.bytes,
                "generation size or line count mismatch"
            );
            Ok(())
        })
        .collect())
}

impl Store {
    pub fn raw_sources(&self, f: &RawFilter) -> Result<Vec<RawSource>> {
        Ok(self
            .raw_generations(f)?
            .into_iter()
            .map(|row| row.source)
            .collect())
    }
    fn raw_generations(&self, f: &RawFilter) -> Result<Vec<SourceGeneration>> {
        let mut q = super::queries::Conditions::new();
        q.sql.push("true".into());
        q.profiles(self, &f.profiles, f.source.as_deref())?;
        if let Some(selector) = &f.selector {
            q.sql.push(
                "(cast(src.id AS VARCHAR)=? OR src.uri=? OR src.source_key=? \
                 OR EXISTS(\
                 SELECT 1 \
                 FROM events e \
                 JOIN sessions s ON s.id=e.session_id \
                 WHERE e.source_id=src.id AND s.profile_id=src.profile_id \
                 AND s.native_id=?))"
                    .into(),
            );
            q.args.extend(std::iter::repeat_n(
                duckdb::types::Value::Text(selector.clone()),
                4,
            ));
        }
        if let Some(generation) = f.generation {
            q.add("g.number=?", generation);
        } else if f.latest {
            q.sql.push(
                "g.number=(SELECT max(g2.number) FROM generations g2 WHERE g2.source_id=src.id )"
                    .into(),
            );
        }
        let harness = self.harness_sql()?;
        let mut st = self.database.connection().prepare(&format!(
            "SELECT src.id,p.name,{harness},src.uri,src.source_key,g.number,\
             g.bytes,g.lines,src.missing,g.id, \
             EXISTS(SELECT 1 FROM captures c WHERE c.generation_id=g.id AND c.encoding='sqlite-rows-jsonl') \
             FROM sources src \
             JOIN profiles p ON p.id=src.profile_id \
             JOIN generations g ON g.source_id=src.id \
             WHERE {} \
             ORDER BY p.name,src.id,g.number",
            q.clause()
        ))?;
        let rows = st
            .query_map(duckdb::params_from_iter(q.args), |r| {
                Ok(SourceGeneration {
                    source: RawSource {
                        id: r.get(0)?,
                        profile: r.get(1)?,
                        source: r.get(2)?,
                        path: PathBuf::from(r.get::<_, String>(3)?),
                        source_key: r.get(4)?,
                        generation: r.get(5)?,
                        bytes: r.get(6)?,
                        lines: r.get(7)?,
                        missing: r.get(8)?,
                    },
                    generation_id: r.get(9)?,
                    sqlite_rows: r.get(10)?,
                })
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        if f.selector.is_some() && rows.is_empty() {
            bail!("no archived source matches the selector in the selected profiles");
        }
        Ok(rows)
    }
    pub fn raw_stats_filtered(&self, f: &RawFilter) -> Result<Value> {
        let sources = self.raw_generations(f)?;
        let ids =
            embedding_batch::id_list(&sources.iter().map(|s| s.generation_id).collect::<Vec<_>>());
        let con = self.database.connection();
        let compressed: u64 = con.query_row(
            "WITH selected AS MATERIALIZED (
                SELECT payload_hash,count(*) n FROM native_observations
                WHERE generation_id IN (SELECT unnest(?::BIGINT[])) GROUP BY payload_hash
             )
             SELECT ((SELECT coalesce(sum(octet_length(p.data)*r.n),0) FROM selected r
                JOIN native_documents p ON p.hash=r.payload_hash)
                + (SELECT coalesce(sum(octet_length(o.data)*r.n),0) FROM selected r
                JOIN content_parts p ON p.document_hash=r.payload_hash
                JOIN content_objects o ON o.id=p.content_id))::UBIGINT",
            [&ids],
            |r| r.get(0),
        )?;
        let raw: u64 = sources.iter().map(|s| s.source.bytes).sum();
        let count = sources
            .iter()
            .map(|s| s.source.id)
            .collect::<std::collections::HashSet<_>>()
            .len();
        Ok(
            json!({"sources":count,"source_generations":sources.len(),"lines":sources.iter().map(|s|s.source.lines).sum::<u64>(),"raw_bytes":raw,"compressed_bytes":compressed,"compression_ratio":if raw==0{0.0}else{compressed as f64/raw as f64}}),
        )
    }
    pub fn verify_raw(&self, f: &RawFilter, against_live: bool, verbose: bool) -> Result<Value> {
        let sources = self.raw_generations(f)?;
        let mut checked = std::collections::HashSet::new();
        let mut details = Vec::new();
        let mut errors = Vec::new();
        let mut bytes = 0;
        let con = self.database.connection();
        for group in archive_data::generation_groups(&sources, SourceGeneration::archive) {
            let results = match verify_group(con, group, against_live, &mut checked) {
                Ok(rows) => rows,
                Err(error) if group.len() == 1 => vec![Err(error)],
                Err(_) => group
                    .iter()
                    .map(|row| {
                        verify_group(con, std::slice::from_ref(row), against_live, &mut checked)
                            .and_then(|mut results| results.remove(0))
                    })
                    .collect(),
            };
            for (row, result) in group.iter().zip(results) {
                let source = &row.source;
                match result {
                    Ok(())=>{bytes+=source.bytes;if verbose{details.push(json!({"source_id":source.id,"profile":source.profile,"generation":source.generation,"verified":true,"live":if against_live{"matches"}else{"not_checked"}}));}},
                    Err(e)=>errors.push(json!({"source_id":source.id,"profile":source.profile,"generation":source.generation,"error":e.to_string()})),
                }
            }
        }
        let mut result = json!({"generations":sources.len(),"bytes":bytes,"payloads":checked.len(),"verified":errors.is_empty(),"errors":errors});
        if verbose {
            result["sources"] = json!(details);
        }
        Ok(result)
    }
}
impl Store {
    /// Writes one generation's bytes. Integrity checks belong to `verify`.
    pub fn export_raw(
        &self,
        source: i64,
        generation: Option<i64>,
        out: &mut impl Write,
    ) -> Result<u64> {
        let con = self.database.connection();
        let (generation, expected, lines): (i64, u64, u64) = con.query_row(
            "SELECT id,bytes,lines FROM generations WHERE source_id=? AND (?::BIGINT IS NULL OR number=?)
             ORDER BY number DESC LIMIT 1",
            params![source, generation, generation],
            |r| Ok((r.get(0)?, r.get(1)?,r.get(2)?)),
        )?;
        let mut size = 0;
        archive_data::visit_generations(
            con,
            &[archive_data::Generation {
                id: generation,
                lines,
                bytes: expected,
            }],
            |_, raw| {
                out.write_all(&raw)?;
                size += raw.len() as u64;
                Ok(())
            },
        )?;
        if size != expected {
            bail!("generation size mismatch");
        }
        Ok(size)
    }
    /// Checks every stored payload once against its xxh64 and that each generation's
    /// records tile its byte range exactly.
    pub fn verify(&self) -> Result<Value> {
        let con = self.database.connection();
        let mut payloads = 0;
        archive_data::visit_documents(con, |record, raw| {
            if xxhash_rust::xxh64::xxh64(&raw, 0) != record.checksum {
                bail!("raw payload integrity error");
            }
            payloads += 1;
            Ok(())
        })?;
        let broken: i64 = self.database.connection().query_row(
            "WITH r AS (SELECT r.generation_id,r.ordinal,r.start_byte,r.end_byte,p.raw_size,
                lag(r.end_byte,1,0) OVER w prev_end,row_number() OVER w n
              FROM native_observations r LEFT JOIN native_documents p ON p.hash=r.payload_hash
              WINDOW w AS (PARTITION BY r.generation_id ORDER BY r.ordinal)),
            g AS (SELECT generation_id,count(*) lines,max(end_byte) bytes,
                count(*) FILTER (\
                 WHERE raw_size IS NULL OR ordinal<>n OR start_byte<>prev_end \
                 OR end_byte-start_byte<>raw_size) bad
              FROM r GROUP BY ALL)
            SELECT count(*) FROM generations x LEFT JOIN g ON g.generation_id=x.id
            WHERE (coalesce(g.bad,0)>0 OR coalesce(g.lines,0)<>x.lines \
             OR coalesce(g.bytes,0)<>x.bytes)",
            [],
            |r| r.get(0),
        )?;
        if broken > 0 {
            bail!("{broken} raw generations do not match their records");
        }
        let (n, bytes): (i64, u64) = self.database.connection().query_row(
            "select count(*),coalesce(sum(bytes),0) from generations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let catalogs_present: bool = self.database.connection().query_row(
            "select count(*)>0 \
             from information_schema.tables \
             where table_schema='main' and table_name='catalog_snapshots'",
            [],
            |r| r.get(0),
        )?;
        let mut catalogs = 0;
        if catalogs_present {
            let mut st = self
                .database
                .connection()
                .prepare("select data,digest from catalog_snapshots")?;
            let mut rows = st.query([])?;
            while let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                let digest: String = row.get(1)?;
                if blake3::hash(data.as_bytes()).to_hex().as_str() != digest {
                    bail!("catalog snapshot digest mismatch");
                }
                let _: llm_history_types::catalog::ProjectCatalog = serde_json::from_str(&data)?;
                catalogs += 1;
            }
        }
        Ok(
            json!({"generations":n,"bytes":bytes,"payloads":payloads,"catalog_snapshots":catalogs,"verified":true}),
        )
    }
}
