use llm_history_ingest::{Presence, tau::Record};

#[test]
fn documented_native_examples_remain_readable_without_schema_round_tripping() {
    let fixtures = [include_str!(
        "../../../docs/harness-history/tau/example.jsonl"
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
fn native_time_and_presence_stay_separate_from_nested_message_fields() {
    let source = r#"{"type":"message","id":"a","parent_id":null,"timestamp":1.25,"message":{"role":"user","timestamp":1250,"content":"hello"},"replaces_entry_ids":[]}"#;
    let record = Record::parse(source).unwrap();
    assert_eq!(record.timestamp().unwrap(), Presence::Value(1.25));
    assert_eq!(record.parent_id().unwrap(), Presence::Null);
    let Presence::Value(message) = record.message().unwrap() else {
        panic!("message object")
    };
    assert_eq!(
        message.field::<u64>("timestamp").unwrap(),
        Presence::Value(1250)
    );
    assert!(
        record
            .extras()
            .any(|(key, value)| key == "replaces_entry_ids" && value.get() == "[]")
    );
    assert!(
        Record::parse(r#"{"timestamp":"1.25"}"#)
            .unwrap()
            .timestamp()
            .is_err()
    );
}
