use llm_history_ingest::content::leaves;
use llm_history_types::Harness;

#[test]
fn codex_exact_tool_arguments_and_future_fields_keep_their_native_spelling() {
    let record = br#"{"type":"response_item","payload":{"type":"function_call","arguments":"{ \"path\": \"hello.txt\" }","unknown":1.000000000000000000001}}"#;
    let leaves = leaves(Harness::Codex, record);
    assert_eq!(leaves.len(), 1);
    let leaf = &leaves[0];
    assert_eq!(leaf.native_path, "/payload/arguments");
    assert_eq!(leaf.body, r#"{ "path": "hello.txt" }"#);
    assert_eq!(
        &record[leaf.token.clone()],
        br#""{ \"path\": \"hello.txt\" }""#
    );
}

#[test]
fn ambiguous_and_noncanonical_leaves_stay_inline() {
    for record in [
        br#"{"type":"user","message":{"content":"First copy long enough","content":"Second copy long enough"}}"#.as_slice(),
        br#"{"type":"user","message":{"content":"An escaped caf\u00e9 string"}}"#,
        br#"{"type":"unknown","message":{"content":"Text in an unknown variant"}}"#,
    ] {
        assert!(leaves(Harness::Claude, record).is_empty());
    }
}

#[test]
fn claude_nested_results_extract_text_without_moving_opaque_blocks() {
    let record = br#"{"type":"user","message":{"content":[{"type":"tool_result","content":[{"type":"text","text":"Reusable tool result"},{"type":"image","source":{"data":"This stays inline"}}]}]}}"#;
    let leaves = leaves(Harness::Claude, record);
    assert_eq!(leaves.len(), 1);
    assert_eq!(leaves[0].native_path, "/message/content/0/content/0/text");
    assert_eq!(leaves[0].body, "Reusable tool result");
}
