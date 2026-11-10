use llm_history_ingest::{JsonRecord, Presence};

#[test]
fn unknown_values_and_original_json_survive_without_numeric_or_text_normalization() {
    let source = " {\"type\":\"future.record\",\"huge\":1234567890123456789012345678901234567890,\"opaque\":{\"type\":\"future.block\",\"n\":1.2300},\"text\":\"\\u0061\"}\n";
    let record = JsonRecord::parse(source).unwrap();
    assert_eq!(record.source(), source);
    assert_eq!(
        record.field::<String>("type").unwrap(),
        Presence::Value("future.record".into())
    );
    let extras = record
        .extras(&["type"])
        .map(|(key, value)| (key, value.get()))
        .collect::<Vec<_>>();
    assert!(extras.contains(&("huge", "1234567890123456789012345678901234567890")));
    assert!(extras.contains(&("opaque", "{\"type\":\"future.block\",\"n\":1.2300}")));
    assert!(extras.contains(&("text", "\"\\u0061\"")));
}

#[test]
fn missing_null_empty_and_wrong_type_have_distinct_outcomes() {
    let source = r#"{"null":null,"empty":[],"changed":{"new":"shape"}}"#;
    let record = JsonRecord::parse(source).unwrap();
    assert_eq!(record.field::<String>("absent").unwrap(), Presence::Missing);
    assert_eq!(record.field::<String>("null").unwrap(), Presence::Null);
    assert_eq!(
        record.field::<Vec<String>>("empty").unwrap(),
        Presence::Value(vec![])
    );
    let error = record.field::<String>("changed").unwrap_err();
    assert_eq!(error.field(), "changed");
    assert_eq!(record.source(), source);
}

#[test]
fn nested_objects_preserve_unrecognized_variants_and_field_presence() {
    let record = JsonRecord::parse(r#"{"message":{"role":"new-role","content":[{"type":"new-block","value":null}]},"nil":null,"array":[]}"#).unwrap();
    let Presence::Value(message) = record.object("message").unwrap() else {
        panic!("message object")
    };
    assert_eq!(
        message.field::<String>("role").unwrap(),
        Presence::Value("new-role".into())
    );
    assert!(message.source().contains(r#""type":"new-block""#));
    assert!(matches!(record.object("nil").unwrap(), Presence::Null));
    assert!(matches!(
        record.object("absent").unwrap(),
        Presence::Missing
    ));
    assert!(record.object("array").is_err());
}

#[test]
fn syntax_errors_and_non_object_records_are_reported() {
    for source in ["[]", "null", "true", "{", "{} {}"] {
        assert!(JsonRecord::parse(source).is_err(), "accepted {source}");
    }
}

#[test]
fn duplicate_key_lookup_keeps_the_complete_source() {
    let source = r#"{"type":"old","type":"new"}"#;
    let record = JsonRecord::parse(source).unwrap();
    assert_eq!(
        record.field::<String>("type").unwrap(),
        Presence::Value("new".into())
    );
    assert_eq!(record.source(), source);
}
