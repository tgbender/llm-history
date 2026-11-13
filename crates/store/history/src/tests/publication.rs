use crate::*;
use llm_history_ingest::{
    prepare::{self, CaptureWork},
    source::discover,
};
use llm_history_types::ingestion::{CaptureKind, PreparedCapture, PreparedRecord};

type Input = (PreparedCapture, Vec<PreparedRecord>);

fn inputs(
    dir: &Path,
    files: usize,
    records_per_file: usize,
    batch_records: usize,
) -> Result<(Store, Profile, Vec<Input>)> {
    let root = dir.join("codex");
    std::fs::create_dir_all(root.join("sessions"))?;
    let store = Store::open_with_options(
        &dir.join("fixture.duckdb"),
        false,
        StoreOptions {
            batch_records,
            ..StoreOptions::default()
        },
    )?;
    let profile = store.add_profile("personal", Harness::Codex, &root, None, None)?;
    for index in 0..files {
        let fixture = llm_history_core::fixture_record(
            profile.harness,
            &format!("thread-{index}"),
            "shared text",
        );
        let mut file = File::create(root.join("sessions").join(format!("{index:03}.jsonl")))?;
        writeln!(file, "{}", fixture[0])?;
        for _ in 1..records_per_file {
            writeln!(file, "{}", fixture[1])?;
        }
    }
    let inputs = prepare_inputs(&store, &profile, false)?;
    assert_eq!(inputs.len(), files);
    Ok((store, profile, inputs))
}

fn prepare_inputs(store: &Store, profile: &Profile, repair: bool) -> Result<Vec<Input>> {
    let checkpoints = store.source_checkpoints(profile)?;
    let mut inputs = Vec::new();
    let index = prepare::CheckpointIndex::new(&checkpoints);
    for source in discover(profile)? {
        let CaptureWork::Read(previous) = index.plan(&source, repair)? else {
            continue;
        };
        let mut records = Vec::new();
        let reader = prepare::open_capture(&source, previous, repair)?;
        assert!(reader.usage_source().is_none());
        let work = reader.read(Default::default(), |r| {
            records.push(r);
            Ok(())
        })?;
        inputs.push((work, records));
    }
    Ok(inputs)
}

#[test]
fn sql_work_tracks_flushes_rather_than_file_count() -> Result<()> {
    let count = |files: usize, limit: usize| -> Result<(i64, Vec<i64>)> {
        let dir = tempfile::tempdir()?;
        let (store, profile, mut inputs) = inputs(dir.path(), files, 96 / files, limit)?;
        let con = store.database.connection();
        con.execute_batch("CALL enable_logging('QueryLog',storage='memory')")?;
        store.publish_captures(
            &profile,
            inputs
                .iter_mut()
                .map(|(work, rows)| (&*work, std::mem::take(rows).into_iter().map(Ok))),
        )?;
        con.execute_batch("CALL disable_logging()")?;
        let mut st = con.prepare("SELECT message FROM duckdb_logs() WHERE type='QueryLog'")?;
        let statements = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let total = con.query_row("SELECT count(*) FROM native_observations", [], |r| r.get(0))?;
        let markers = [
            "create temp table cas_new_documents as",
            "create or replace temp table stage_canonical as",
            "insert into sessions",
            "insert into sources",
            "create or replace temp table finished_sources",
        ];
        Ok((
            total,
            markers
                .iter()
                .map(|marker| {
                    statements
                        .iter()
                        .filter(|sql| sql.to_ascii_lowercase().contains(marker))
                        .count() as i64
                })
                .collect(),
        ))
    };
    let one_flush = count(12, 256)?;
    assert_eq!(one_flush, (96, vec![1, 1, 1, 1, 1]));
    assert_eq!(count(48, 256)?, one_flush);
    assert_eq!(count(48, 48)?, (96, vec![2, 2, 2, 1, 1]));
    Ok(())
}

#[test]
fn failure_after_shared_flush_rolls_back_the_group_and_allows_retry() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (store, profile, inputs) = inputs(dir.path(), 3, 3, 4)?;
    let con = store.database.connection();
    let frames: Vec<Vec<_>> = inputs
        .iter()
        .map(|(_, rows)| {
            rows.iter()
                .map(serde_json::to_vec)
                .collect::<serde_json::Result<_>>()
        })
        .collect::<serde_json::Result<_>>()?;
    let error = store
        .publish_captures(
            &profile,
            inputs
                .iter()
                .zip(&frames)
                .enumerate()
                .map(|(idx, ((work, _), rows))| {
                    (
                        work,
                        rows.iter().map(move |r| {
                            if idx == 2 {
                                // First two sources have already crossed the flush limit.
                                let count: i64 = con.query_row(
                                    "SELECT count(*) FROM native_observations",
                                    [],
                                    |r| r.get(0),
                                )?;
                                assert_eq!(count, 4);
                                bail!("injected failure");
                            }
                            Ok(serde_json::from_slice(r)?)
                        }),
                    )
                }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("injected failure"));
    for table in [
        "sources",
        "generations",
        "captures",
        "native_observations",
        "native_documents",
        "content_objects",
        "events",
        "lexical_documents",
        "sessions",
        "canonical_sessions",
    ] {
        let count: i64 =
            con.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
        assert_eq!(count, 0, "{table}");
    }
    store.publish_captures(
        &profile,
        inputs.iter().zip(&frames).map(|((work, _), rows)| {
            (
                work,
                rows.iter().map(|frame| Ok(serde_json::from_slice(frame)?)),
            )
        }),
    )?;
    assert_eq!(store.source_checkpoints(&profile)?.len(), 3);
    Ok(())
}

#[test]
fn mixed_batch_preserves_sequences_and_identical_repair_keeps_raw_ids() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("codex");
    let sessions = root.join("sessions");
    std::fs::create_dir_all(&sessions)?;
    let store = Store::open(&dir.path().join("fixture.duckdb"), false)?;
    let profile = store.add_profile("personal", Harness::Codex, &root, None, None)?;
    let fixture = |text| llm_history_core::fixture_record(Harness::Codex, "one-thread", text);
    let write = |name: &str, text| {
        let records = fixture(text);
        std::fs::write(
            sessions.join(name),
            format!("{}\n{}\n", records[0], records[1]),
        )
    };
    let publish = |repair| {
        store.publish_captures(
            &profile,
            prepare_inputs(&store, &profile, repair)?
                .iter_mut()
                .map(|(work, rows)| (&*work, std::mem::take(rows).into_iter().map(Ok))),
        )
    };
    let state = || -> Result<Vec<(i64, i64, i64, i64)>> {
        let mut st = store.database.connection().prepare(
            "SELECT s.generation,cs.sequence_base,e.id,e.raw_record_id
             FROM sources s JOIN canonical_session_sources cs ON cs.source_id=s.id AND cs.active
             JOIN events e ON e.source_id=s.id AND e.ordinal=2 AND e.fact_index=0 AND e.active
             ORDER BY s.source_key",
        )?;
        Ok(st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<duckdb::Result<_>>()?)
    };
    write("a.jsonl", "source a")?;
    write("b.jsonl", "source b before rewrite")?;
    let old_b = std::fs::read(sessions.join("b.jsonl"))?;
    assert_eq!(publish(false)?, vec![(CaptureKind::New, 2); 2]);
    let before = state()?;
    writeln!(
        OpenOptions::new()
            .append(true)
            .open(sessions.join("a.jsonl"))?,
        "{}",
        fixture("append to a")[1]
    )?;
    write("b.jsonl", "b is now a different length")?;
    write("c.jsonl", "source c")?;
    assert_eq!(
        publish(false)?,
        [
            (CaptureKind::Append, 1),
            (CaptureKind::Rewrite, 2),
            (CaptureKind::New, 2),
        ]
    );
    let mixed = state()?;
    assert_eq!(mixed[0], before[0]);
    assert_eq!(mixed[1].0, 2);
    assert_eq!(mixed[1].1, before[1].1);
    assert_eq!(mixed[2].1, 2_i64 << 32);
    let checkpoints = store.source_checkpoints(&profile)?;
    let mut exported = Vec::new();
    store.export_raw(
        checkpoints
            .iter()
            .find(|s| Path::new(&s.key).ends_with("b.jsonl"))
            .unwrap()
            .id,
        Some(1),
        &mut exported,
    )?;
    assert_eq!(exported, old_b);
    assert_eq!(publish(true)?.len(), 3);
    assert_eq!(state()?, mixed);
    Ok(())
}

#[test]
fn compaction_epochs_carry_across_flushes_and_seed_once_on_append() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("codex");
    let sessions = root.join("sessions");
    std::fs::create_dir_all(&sessions)?;
    let db = dir.path().join("fixture.duckdb");
    let options = StoreOptions {
        batch_records: 3,
        ..StoreOptions::default()
    };
    let store = Store::open_with_options(&db, false, options)?;
    let profile = store.add_profile("personal", Harness::Codex, &root, None, None)?;
    let fixture = llm_history_core::fixture_record(Harness::Codex, "shared-thread", "same text");
    let (header, message) = (&fixture[0], &fixture[1]);
    let compact = json!({"type":"compacted","payload":{"message":"summary"}});
    let a = sessions.join("a.jsonl");
    let b = sessions.join("b.jsonl");
    // The first compaction ends a flush. The second is followed by a raw-only
    // line, so the source checkpoint ordinal cannot locate the last event.
    std::fs::write(
        &a,
        format!("{header}\n{message}\n{compact}\n{message}\n{compact}\nnot json\n"),
    )?;
    std::fs::write(&b, format!("{header}\n{message}\n"))?;
    let publish = |store: &Store| {
        store.publish_captures(
            &profile,
            prepare_inputs(store, &profile, false)?
                .iter_mut()
                .map(|(work, rows)| (&*work, std::mem::take(rows).into_iter().map(Ok))),
        )
    };
    publish(&store)?;
    drop(store);

    writeln!(
        OpenOptions::new().append(true).open(&a)?,
        "not json either\n{message}\n{compact}\n{message}\n{compact}\n{compact}\n{message}"
    )?;
    writeln!(OpenOptions::new().append(true).open(&b)?, "{message}")?;
    let store = Store::open_with_options(&db, false, options)?;
    let con = store.database.connection();
    con.execute_batch("CALL enable_logging('QueryLog',storage='memory')")?;
    assert_eq!(
        publish(&store)?,
        [(CaptureKind::Append, 7), (CaptureKind::Append, 1)]
    );
    con.execute_batch("CALL disable_logging()")?;
    let work: (i64, i64, i64) = con.query_row(
        "SELECT count(*) FILTER (WHERE message ILIKE '%WITH append_epoch_ends AS MATERIALIZED%'),
                count(*) FILTER (WHERE message ILIKE '%CREATE OR REPLACE TEMP TABLE stage_canonical AS%'),
                count(*) FILTER (WHERE message ILIKE '%stage_canonical_prefix%')
         FROM duckdb_logs() WHERE type='QueryLog'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    assert_eq!(
        work,
        (1, 3, 0),
        "one append seed for three projection flushes"
    );

    let projected = |store: &Store| {
        let mut st = store.database.connection().prepare(
            "SELECT s.source_key,e.ordinal,ep.ordinal-css.sequence_base,e.id,ep.id
             FROM events e JOIN sources s ON s.id=e.source_id
             JOIN canonical_context_epochs ep ON ep.id=e.context_epoch_id
             JOIN canonical_session_sources css ON css.session_id=e.session_id AND css.source_id=e.source_id
             WHERE e.active AND e.fact_index=0 ORDER BY s.source_key,e.ordinal",
        )?;
        Ok::<_, anyhow::Error>(
            st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?,
        )
    };
    let actual = projected(&store)?;
    let epoch_order = |rows: &[(String, u64, i64, i64, i64)]| {
        rows.iter()
            .map(|(key, ordinal, epoch, _, _)| {
                (Path::new(key).ends_with("a.jsonl"), *ordinal, *epoch)
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        epoch_order(&actual),
        [
            (true, 1, 0),
            (true, 2, 0),
            (true, 3, 0),
            (true, 4, 1),
            (true, 5, 1),
            (true, 8, 2),
            (true, 9, 2),
            (true, 10, 3),
            (true, 11, 3),
            (true, 12, 4),
            (true, 13, 5),
            (false, 1, 0),
            (false, 2, 0),
            (false, 3, 0),
        ]
    );
    let replay = crate::tests::fixtures::replay(&store, &[], None, &[], false)?;
    assert!(replay.errors.is_empty(), "{:?}", replay.errors);
    assert_eq!(
        projected(&store)?,
        actual,
        "replay preserves event and epoch IDs"
    );

    let single = Store::open(&dir.path().join("single.duckdb"), false)?;
    let single_profile = single.add_profile("personal", Harness::Codex, &root, None, None)?;
    single.publish_captures(
        &single_profile,
        prepare_inputs(&single, &single_profile, false)?
            .iter_mut()
            .map(|(work, rows)| (&*work, std::mem::take(rows).into_iter().map(Ok))),
    )?;
    assert_eq!(epoch_order(&projected(&single)?), epoch_order(&actual));

    // Replacing a generation resets the cursor. Its superseded epochs must
    // stay retired when the replacement is appended to.
    std::fs::write(&a, format!("{header}\n{message}\n"))?;
    assert_eq!(publish(&store)?, [(CaptureKind::Rewrite, 2)]);
    writeln!(OpenOptions::new().append(true).open(&a)?, "{message}")?;
    assert_eq!(publish(&store)?, [(CaptureKind::Append, 1)]);
    assert_eq!(
        epoch_order(&projected(&store)?),
        [
            (true, 1, 0),
            (true, 2, 0),
            (true, 3, 0),
            (false, 1, 0),
            (false, 2, 0),
            (false, 3, 0),
        ]
    );
    Ok(())
}

#[test]
fn replay_and_verify_use_one_archive_query_pair_per_group() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (store, profile, mut inputs) = inputs(dir.path(), 48, 2, 256)?;
    store.publish_captures(
        &profile,
        inputs
            .iter_mut()
            .map(|(work, rows)| (&*work, std::mem::take(rows).into_iter().map(Ok))),
    )?;
    let con = store.database.connection();
    con.execute_batch("CALL enable_logging('QueryLog',storage='memory')")?;
    let selected = store.replay_sources(&profile, &[])?;
    let mut rows = vec![0; selected.len()];
    store.retained_records(&selected.iter().collect::<Vec<_>>(), |i, r| {
        rows[i] += 1;
        assert_eq!(r.ordinal, rows[i]);
        assert!(r.raw_id.is_some());
        Ok(())
    })?;
    assert_eq!(rows, vec![2; 48]);
    assert_eq!(
        store.verify_raw(&RawFilter::default(), false, false)?["verified"],
        true
    );
    con.execute_batch("CALL disable_logging()")?;
    let counts: (i64,i64)=con.query_row(
        "SELECT count(*) FILTER(WHERE message ILIKE '%LEFT JOIN native_documents p USING (hash)%'),
                count(*) FILTER(WHERE message ILIKE '%JOIN content_parts c ON c.document_hash=s.hash%')
         FROM duckdb_logs() WHERE type='QueryLog'",
        [], |r| Ok((r.get(0)?,r.get(1)?)),
    )?;
    // DuckDB logs both prepare and execute for each parameterized query.
    assert_eq!(
        counts,
        (4, 4),
        "one archive query pair each for replay and verification"
    );
    Ok(())
}

#[test]
fn graph_dirty_survives_reopen_and_clean_reconciliation_skips_graph_work() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (store, profile, mut inputs) = inputs(dir.path(), 2, 3, 256)?;
    store.publish_captures(
        &profile,
        inputs
            .iter_mut()
            .map(|(work, rows)| (&*work, std::mem::take(rows).into_iter().map(Ok))),
    )?;
    let other_root = dir.path().join("other");
    std::fs::create_dir_all(other_root.join("sessions"))?;
    let other = store.add_profile("other", Harness::Codex, &other_root, None, None)?;
    let fixture = llm_history_core::fixture_record(Harness::Codex, "other-thread", "other text");
    std::fs::write(
        other_root.join("sessions/a.jsonl"),
        format!("{}\n{}\n", fixture[0], fixture[1]),
    )?;
    store.publish_captures(
        &other,
        prepare_inputs(&store, &other, false)?
            .iter_mut()
            .map(|(work, rows)| (&*work, std::mem::take(rows).into_iter().map(Ok))),
    )?;
    let db = store.path.clone();
    drop(store);
    let store = Store::open(&db, false)?;
    store.reconcile_conversations(false)?;
    let con = store.database.connection();
    let memberships: i64 = con.query_row(
        "SELECT count(*) FROM canonical_conversation_memberships",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(memberships, 3);
    con.execute(
        "UPDATE canonical_conversation_memberships SET evidence_kind='other profile sentinel'
         WHERE session_id IN (SELECT id FROM canonical_sessions WHERE store_id=?)",
        [&other.id],
    )?;
    con.execute(
        "INSERT INTO meta VALUES (?, '')",
        [format!("conversation_graph_pending:{}", profile.id)],
    )?;
    store.reconcile_conversations(false)?;
    let other_evidence: String = con.query_row(
        "SELECT m.evidence_kind FROM canonical_conversation_memberships m
         JOIN canonical_sessions s ON s.id=m.session_id WHERE s.store_id=?",
        [&other.id],
        |r| r.get(0),
    )?;
    assert_eq!(other_evidence, "other profile sentinel");
    con.execute_batch("CALL enable_logging('QueryLog',storage='memory')")?;
    store.reconcile_conversations(false)?;
    con.execute_batch("CALL disable_logging()")?;
    let graph_statements: i64 = con.query_row(
        "SELECT count(*) FROM duckdb_logs() WHERE type='QueryLog' AND message ILIKE '%canonical_%'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(graph_statements, 0);
    Ok(())
}

#[test]
fn repeated_documents_do_not_reprobe_cas_after_the_first_flush() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (store, profile, mut inputs) = inputs(dir.path(), 1, 10, 2)?;
    let con = store.database.connection();
    con.execute_batch("CALL enable_logging('QueryLog',storage='memory')")?;
    store.publish_captures(
        &profile,
        inputs
            .iter_mut()
            .map(|(work, rows)| (&*work, std::mem::take(rows).into_iter().map(Ok))),
    )?;
    con.execute_batch("CALL disable_logging()")?;
    let scans: i64 = con.query_row(
        "SELECT count(*) FROM duckdb_logs()
         WHERE type='QueryLog' AND message ILIKE '%CREATE TEMP TABLE cas_new_documents AS%'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(scans, 1);
    let mut bytes = Vec::new();
    let source = store.source_checkpoints(&profile)?.remove(0);
    store.export_raw(source.id, None, &mut bytes)?;
    assert_eq!(bytes, std::fs::read(source.uri)?);
    Ok(())
}

#[test]
fn oversized_generation_is_read_in_order_and_exports_exactly() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (store, profile, mut inputs) = inputs(dir.path(), 1, 20_002, 20_000)?;
    store.publish_captures(
        &profile,
        inputs
            .iter_mut()
            .map(|(work, rows)| (&*work, std::mem::take(rows).into_iter().map(Ok))),
    )?;
    let selected = store.replay_sources(&profile, &[])?;
    let mut ordinal = 0;
    store.retained_records(&[&selected[0]], |_, r| {
        ordinal += 1;
        assert_eq!(r.ordinal, ordinal);
        Ok(())
    })?;
    assert_eq!(ordinal, 20_002);
    let mut bytes = Vec::new();
    store.export_raw(selected[0].checkpoint.id, None, &mut bytes)?;
    assert_eq!(bytes, std::fs::read(&selected[0].checkpoint.uri)?);
    Ok(())
}
