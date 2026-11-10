use crate::*;
fn setup() -> Result<(tempfile::TempDir, Store, Profile)> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("codex");
    std::fs::create_dir_all(root.join("sessions"))?;
    let s = Store::open_with_options(
        &d.path().join("test.duckdb"),
        false,
        StoreOptions {
            batch_records: 7,
            ..StoreOptions::default()
        },
    )?;
    let p = s.add_profile("personal", Harness::Codex, &root, None, None)?;
    Ok((d, s, p))
}
fn history(p: &Profile, text: &str) -> Result<PathBuf> {
    let path = p.root.join("sessions/a.jsonl");
    let records = llm_history_core::fixture_record(p.harness, "same-native", text);
    std::fs::write(
        &path,
        records.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )?;
    Ok(path)
}
#[test]
fn runtime_transaction_and_raw_roundtrip() -> Result<()> {
    let (_d, s, p) = setup()?;
    assert_eq!(
        s.status(&[], None, llm_history_ingest::PARSER_VERSION)?["duckdb"],
        "v2.0.0"
    );
    let path = history(&p, "unicode 🦀\nbyte exact")?;
    crate::tests::fixtures::capture(&s, &[], false)?;
    let id = s
        .database
        .connection()
        .query_row("select id from sources", [], |r| r.get(0))?;
    let mut raw = Vec::new();
    s.export_raw(id, None, &mut raw)?;
    assert_eq!(raw, std::fs::read(path)?);
    s.verify()?;
    Ok(())
}
#[test]
fn append_and_full_converge() -> Result<()> {
    let (_d, s, p) = setup()?;
    let path = history(&p, "first")?;
    crate::tests::fixtures::capture(&s, &[], false)?;
    let ids = s.messages_page(&Filter::default(), None, usize::MAX)?;
    let mut f = OpenOptions::new().append(true).open(&path)?;
    writeln!(
        f,
        "{}",
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":"second"}})
    )?;
    assert_eq!(crate::tests::fixtures::capture(&s, &[], false)?.appended, 1);
    assert_eq!(
        crate::tests::fixtures::capture(&s, &[], false)?.unchanged,
        1
    );
    assert_eq!(
        s.messages_page(&Filter::default(), None, usize::MAX)?[0].id,
        ids[0].id
    );
    s.verify()?;
    Ok(())
}

#[test]
fn growing_transcript_stops_at_cutoff_and_stale_work_cannot_publish() -> Result<()> {
    use llm_history_ingest::{prepare, source::discover};

    let (_d, store, profile) = setup()?;
    let path = history(&profile, "initial")?;
    let original = std::fs::read(&path)?;
    let mut writer = OpenOptions::new().append(true).open(&path)?;
    let mut records = Vec::new();
    let source = discover(&profile)?.remove(0);
    let prepared =
        prepare::open_capture(&source, None, false)?.read(Default::default(), |record| {
            records.push(record);
            writeln!(writer, "{}", json!({"type":"future_record"}))?;
            Ok(())
        })?;
    store.publish_captures(&profile, [(&prepared, records.into_iter().map(Ok))])?;
    let checkpoint = store.source_checkpoints(&profile)?.remove(0);
    let mut raw = Vec::new();
    store.export_raw(checkpoint.id, None, &mut raw)?;
    assert_eq!(raw, original);

    crate::tests::fixtures::capture(&store, &[], false)?;
    let error = store
        .publish_captures(&profile, [(&prepared, std::iter::empty())])
        .unwrap_err();
    assert!(error.to_string().contains("checkpoint changed"));
    raw.clear();
    store.export_raw(checkpoint.id, None, &mut raw)?;
    assert_eq!(raw, std::fs::read(path)?);
    store.verify()?;
    Ok(())
}

#[test]
fn batched_import_spans_flushes_and_keeps_ids() -> Result<()> {
    let (_d, s, p) = setup()?;
    let path = history(&p, "start")?;
    let line = |i: usize| json!({"type":"response_item","payload":{"type":"message","role":"user","content":format!("line {}", i % 10)}});
    let mut f = OpenOptions::new().append(true).open(&path)?;
    for i in 0..50 {
        writeln!(f, "{}", line(i))?;
    }
    crate::tests::fixtures::capture(&s, &[], false)?;
    let count = |s: &Store, sql: &str| -> Result<i64> {
        Ok(s.database.connection().query_row(sql, [], |r| r.get(0))?)
    };
    let before = s.messages_page(&Filter::default(), None, usize::MAX)?;
    assert_eq!(before.len(), 51);
    assert_eq!(count(&s, "select count(*) from lexical_documents")?, 51);
    // Repeated lines share one payload; IDs ascend with file order.
    assert_eq!(
        count(&s, "select count(*) from native_documents")?,
        count(
            &s,
            "select count(distinct payload_hash) from native_observations"
        )?
    );
    assert_eq!(
        count(
            &s,
            "select count(*) \
             from (\
             select id, row_number() over (order by id) a, row_number() over (order by ordinal) b \
             from events) \
             where a<>b"
        )?,
        0
    );
    for i in 50..70 {
        writeln!(f, "{}", line(i))?;
    }
    assert_eq!(crate::tests::fixtures::capture(&s, &[], false)?.appended, 1);
    let after = s.messages_page(&Filter::default(), None, usize::MAX)?;
    assert_eq!(after.len(), 71);
    for m in &before {
        assert!(after.iter().any(|a| a.id == m.id && a.text == m.text));
    }
    let ids: Vec<i64> = {
        let mut st = s
            .database
            .connection()
            .prepare("select id from events order by id")?;
        st.query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?
    };
    crate::tests::fixtures::replay(&s, &[], None, &[], false)?;
    let mut st = s
        .database
        .connection()
        .prepare("select id from events where active order by id")?;
    let again: Vec<i64> = st
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    assert_eq!(ids, again);
    let mut raw = Vec::new();
    s.export_raw(
        s.database
            .connection()
            .query_row("select id from sources", [], |r| r.get(0))?,
        None,
        &mut raw,
    )?;
    assert_eq!(raw, std::fs::read(&path)?);
    s.verify()?;
    Ok(())
}
#[test]
fn profiles_never_cross_context_or_projects() -> Result<()> {
    let (d, s, p) = setup()?;
    history(&p, "personal")?;
    let root = d.path().join("work");
    std::fs::create_dir_all(root.join("sessions"))?;
    let q = s.add_profile("work", Harness::Codex, &root, None, None)?;
    history(&q, "work")?;
    crate::tests::fixtures::capture(&s, &[], false)?;
    let m = s.messages_page(
        &Filter {
            profiles: vec!["personal".into()],
            ..Filter::default()
        },
        None,
        usize::MAX,
    )?;
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].text, "personal");
    assert_eq!(s.context(m[0].message_no, 100, 100, &[])?.len(), 1);
    assert!(
        s.context(m[0].message_no, 100, 100, &["work".into()])
            .is_err()
    );
    Ok(())
}
#[test]
fn missing_retained_and_relocation_keeps_ids() -> Result<()> {
    let (d, s, p) = setup()?;
    history(&p, "keep")?;
    crate::tests::fixtures::capture(&s, &[], false)?;
    let before = s.messages_page(&Filter::default(), None, usize::MAX)?[0]
        .id
        .clone();
    let root = d.path().join("moved");
    std::fs::rename(&p.root, &root)?;
    s.relocate("personal", &root)?;
    crate::tests::fixtures::capture(&s, &[], false)?;
    assert_eq!(
        before,
        s.messages_page(&Filter::default(), None, usize::MAX)?[0].id
    );
    std::fs::remove_dir_all(root)?;
    assert_eq!(crate::tests::fixtures::capture(&s, &[], false)?.missing, 1);
    assert_eq!(
        s.messages_page(&Filter::default(), None, usize::MAX)?.len(),
        1
    );
    Ok(())
}
#[test]
fn rewrite_retains_generations() -> Result<()> {
    let (_d, s, p) = setup()?;
    history(&p, "old")?;
    crate::tests::fixtures::capture(&s, &[], false)?;
    history(&p, "replacement")?;
    crate::tests::fixtures::capture(&s, &[], false)?;
    assert_eq!(
        s.messages_page(&Filter::default(), None, usize::MAX)?[0].text,
        "replacement"
    );
    assert_eq!(s.verify()?["generations"], 2);
    Ok(())
}
#[test]
fn reader_writer_exclusion() -> Result<()> {
    let (d, s, _p) = setup()?;
    assert!(Store::open(&d.path().join("test.duckdb"), true).is_err());
    drop(s);
    let r = Store::open(&d.path().join("test.duckdb"), true)?;
    assert!(Store::open(&d.path().join("test.duckdb"), false).is_err());
    drop(r);
    Ok(())
}
