use llm_history_ingest::{Presence, codex::Record};

#[test]
fn documented_native_examples_remain_readable_without_schema_round_tripping() {
    let fixtures = [
        include_str!("../../../docs/harness-history/codex/examples/conversation.jsonl"),
        include_str!("../../../docs/harness-history/codex/examples/lineage-and-compaction.jsonl"),
    ];
    for source in fixtures
        .into_iter()
        .flat_map(str::lines)
        .filter(|line| !line.trim().is_empty())
    {
        let record = Record::parse(source).unwrap();
        assert!(matches!(record.record_type().unwrap(), Presence::Value(_)));
        assert_eq!(record.native().source(), source);
    }
}

#[test]
fn future_record_kind_and_opaque_fields_are_preserved() {
    let source = r#"{"type":"future.record","vendor_extension":{"type":"future.block","value":null,"number":1.2300}}"#;
    let record = Record::parse(source).unwrap();
    assert_eq!(
        record.record_type().unwrap(),
        Presence::Value("future.record".into())
    );
    assert_eq!(record.native().source(), source);
    assert!(
        record
            .extras()
            .any(|(key, value)| key == "vendor_extension" && value.get().contains("1.2300"))
    );
}

#[test]
fn future_payload_variants_survive_and_changed_envelope_types_are_explicit() {
    let source = r#"{"type":"response_item","ordinal":7,"payload":{"type":"future.item","opaque":[null,{"new":true}]},"metadata":null}"#;
    let record = Record::parse(source).unwrap();
    assert_eq!(record.ordinal().unwrap(), Presence::Value(7));
    assert!(matches!(record.metadata().unwrap(), Presence::Null));
    let Presence::Value(payload) = record.payload().unwrap() else {
        panic!("payload object")
    };
    assert_eq!(
        payload.field::<String>("type").unwrap(),
        Presence::Value("future.item".into())
    );
    let changed = Record::parse(r#"{"type":{"new":"shape"},"payload":[]}"#).unwrap();
    assert!(changed.record_type().is_err());
    assert!(changed.payload().is_err());
    assert!(changed.native().source().contains("new"));
}
