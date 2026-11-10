use llm_history_ingest::{Presence, claude_code::Record};

#[test]
fn documented_native_examples_remain_readable_without_schema_round_tripping() {
    let fixtures = [
        include_str!("../../../docs/harness-history/claude-code/examples/parallel-tools.jsonl"),
        include_str!(
            "../../../docs/harness-history/claude-code/examples/compaction-and-fork.jsonl"
        ),
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
fn native_entry_response_and_tool_ids_remain_distinct() {
    let record = Record::parse(r#"{"type":"assistant","uuid":"entry","parentUuid":null,"sessionId":"session","message":{"id":"response","role":"assistant","content":[{"type":"tool_use","id":"call","input":{}}]},"isMeta":false}"#).unwrap();
    assert_eq!(record.entry_id().unwrap(), Presence::Value("entry".into()));
    assert_eq!(record.parent_id().unwrap(), Presence::Null);
    assert!(matches!(record.timestamp().unwrap(), Presence::Missing));
    let Presence::Value(message) = record.message().unwrap() else {
        panic!("message object")
    };
    assert_eq!(
        message.field::<String>("id").unwrap(),
        Presence::Value("response".into())
    );
    assert!(
        record
            .extras()
            .any(|(key, value)| key == "isMeta" && value.get() == "false")
    );
}
