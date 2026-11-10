use llm_history_ingest::{Presence, pi::Record};

#[test]
fn documented_native_examples_remain_readable_without_schema_round_tripping() {
    let fixtures = [include_str!(
        "../../../docs/harness-history/pi/example.jsonl"
    )];
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
fn header_and_message_shapes_keep_their_native_fields() {
    let header = Record::parse(r#"{"type":"session","version":3,"id":"session","timestamp":"2026-11-07T00:00:00Z","cwd":"/example"}"#).unwrap();
    assert_eq!(header.version().unwrap(), Presence::Value(3));
    assert!(matches!(header.message().unwrap(), Presence::Missing));
    assert!(header.extras().any(|(key, _)| key == "cwd"));
    let entry = Record::parse(r#"{"type":"message","parentId":null,"timestamp":"2026-11-07T00:00:01Z","message":{"timestamp":1794009601000,"role":"future-role"}}"#).unwrap();
    assert_eq!(entry.parent_id().unwrap(), Presence::Null);
    let Presence::Value(message) = entry.message().unwrap() else {
        panic!("message object")
    };
    assert_eq!(
        message.field::<String>("role").unwrap(),
        Presence::Value("future-role".into())
    );
}
