use anyhow::Result;
use llm_history_ingest::ParserState;
use llm_history_types::Harness;
use serde_json::{Value, json};
use std::path::Path;

#[test]
fn provider_facts_match_frozen_python_fixtures() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let golden: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("reference.json"))?)?;
    for expected in golden["sources"].as_array().unwrap() {
        let harness: Harness = expected["harness"].as_str().unwrap().parse()?;
        let path = root.join(expected["file"].as_str().unwrap());
        let mut parser = ParserState::new(&path, harness);
        let mut messages = Vec::new();
        let mut tools = 0;
        let mut results = 0;
        let mut input = 0;
        let mut output = 0;
        for (i, line) in std::fs::read_to_string(&path)?.lines().enumerate() {
            for fact in parser.parse(harness, &serde_json::from_str(line)?) {
                if fact.kind == "message" && !fact.text.is_empty() {
                    messages.push(
                        json!({"line_no":i+1,"role":fact.role,"phase":fact.phase,"text":fact.text}),
                    );
                }
                tools += usize::from(fact.kind == "tool_call");
                results += usize::from(fact.kind == "tool_result");
                input += fact.usage.input;
                output += fact.usage.output;
            }
        }
        assert_eq!(
            json!(messages),
            expected["messages"],
            "{} messages",
            harness.key()
        );
        assert_eq!(json!(parser.session), expected["native_session_id"]);
        assert_eq!(json!(tools), expected["tools"]);
        assert_eq!(json!(results), expected["tool_results"]);
        assert_eq!(json!(input), expected["input_tokens"]);
        assert_eq!(json!(output), expected["output_tokens"]);
    }
    Ok(())
}
#[test]
fn subagent_namespace_matches_python_and_retains_parent() {
    let mut parser = ParserState::new(
        Path::new("projects/p/session/subagents/agent-42.jsonl"),
        Harness::Claude,
    );
    parser.parse(Harness::Claude,&json!({"type":"assistant","sessionId":"session","message":{"content":"done","stop_reason":"end_turn"}}));
    assert_eq!(parser.session, "claude:session:42");
    assert_eq!(parser.parent, "claude:session");
    assert_eq!(parser.relation, "spawned");
}
