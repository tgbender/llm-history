use crate::{EmbeddingBatch, EmbeddingDocument, Store};
use anyhow::Result;
use llm_history_core::fixture_record;
use llm_history_types::Harness;

#[test]
fn pending_pages_filter_before_reading_text_and_skip_coverage_failures_and_oversize() -> Result<()>
{
    let d = tempfile::tempdir()?;
    let store = Store::open(&d.path().join("pending.duckdb"), false)?;
    for name in ["personal", "work"] {
        let root = d.path().join(name);
        std::fs::create_dir_all(root.join("sessions"))?;
        store.add_profile(name, Harness::Codex, &root, None, None)?;
        for i in 0..19 {
            std::fs::write(
                root.join(format!("sessions/{i:02}.jsonl")),
                fixture_record(
                    Harness::Codex,
                    &format!("thread-{i}"),
                    &format!("{name} {i}"),
                )
                .iter()
                .map(|r| format!("{r}\n"))
                .collect::<String>(),
            )?;
        }
        std::fs::write(
            root.join("sessions/large.jsonl"),
            fixture_record(Harness::Codex, "large", &"x".repeat(2000))
                .iter()
                .map(|r| format!("{r}\n"))
                .collect::<String>(),
        )?;
    }
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    assert!(store.has_pending_messages("model", &["work".into()], Some("codex"))?);
    assert!(!store.has_pending_messages("model", &["work".into()], Some("claude"))?);
    let first = store.pending_messages("model", &["work".into()], None, i64::MIN, 1, 100, &[])?;
    let excluded = first.messages[0].id.parse::<i64>()?;
    let page = store.pending_messages(
        "model",
        &["work".into()],
        None,
        i64::MIN,
        19,
        100,
        &[excluded],
    )?;
    assert_eq!(page.messages.len(), 18);
    assert!(page.messages.iter().all(|m| m.id != excluded.to_string()));
    let mut after = i64::MIN;
    let mut ids = Vec::new();
    loop {
        let page =
            store.pending_messages("model", &["work".into()], Some("codex"), after, 3, 100, &[])?;
        assert!(page.messages.len() <= 3);
        if page.messages.is_empty() {
            // Oversized documents are counted once, on the page that ends the scan.
            assert_eq!(page.oversized, 1);
            break;
        }
        assert_eq!(page.oversized, 0);
        after = page.last_id;
        let mut failures = Vec::new();
        for message in page.messages {
            assert_eq!(message.profile, "work");
            assert!(message.text.len() < 100);
            ids.push(message.id.clone());
            failures.push(EmbeddingDocument {
                id: message.id.parse()?,
                text_hash: message.text_hash,
                vectors: Vec::new(),
                failure: Some("test".into()),
            });
        }
        store.publish_embedding_batch("model", "{}", &EmbeddingBatch::new(&failures)?)?;
    }
    assert_eq!(ids.len(), 19);
    let large = store.pending_messages("model", &["work".into()], None, i64::MIN, 1, 3000, &[])?;
    store.publish_embedding_batch(
        "model",
        "{}",
        &EmbeddingBatch::new(&[EmbeddingDocument {
            id: large.messages[0].id.parse()?,
            text_hash: large.messages[0].text_hash.clone(),
            vectors: Vec::new(),
            failure: Some("test".into()),
        }])?,
    )?;
    assert!(!store.has_pending_messages("model", &["work".into()], None)?);
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        19
    );
    assert!(
        store
            .pending_messages(
                "model",
                &["work".into()],
                Some("claude"),
                i64::MIN,
                3,
                100,
                &[]
            )?
            .messages
            .is_empty()
    );
    assert!(
        store
            .pending_messages("model", &["unknown".into()], None, i64::MIN, 3, 100, &[])
            .is_err()
    );
    Ok(())
}
