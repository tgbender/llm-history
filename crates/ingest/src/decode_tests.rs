use crate::ParserState;
use llm_history_core::fixture_record;
use llm_history_types::Harness;
use serde_json::{Value, json};
use std::path::Path;

#[test]
fn request_logs_never_replay() {
    let mut p = ParserState::new(Path::new("fake"), Harness::Claude);
    let f = p.parse(
        Harness::Claude,
        &json!({"type":"api-request-blob","message":{"content":"secret replay"}}),
    );
    assert_eq!(f[0].kind, "request_log");
    assert!(f[0].text.is_empty());
}

#[test]
fn parallel_tools_are_unique() {
    let mut p = ParserState::default();
    let f=p.parse(Harness::Claude,&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"a"},{"type":"tool_use","id":"b"}]}}));
    assert_eq!(f.len(), 3);
    assert_ne!(f[1].index, f[2].index);
}

#[test]
fn claude_sessions_start_at_their_earliest_record() {
    let mut p = ParserState::default();
    for ts in ["2026-10-19T17:29:50.373Z", "2026-10-19T16:53:02.405Z"] {
        p.parse(
            Harness::Claude,
            &json!({"type":"queue-operation","sessionId":"s","timestamp":ts}),
        );
    }
    assert_eq!(p.canonical.started, "2026-10-19T16:53:02.405Z");
}

#[test]
fn only_guardian_prompts_search_their_action() {
    let prompt = "[1] user: hello\n>>> APPROVAL REQUEST START\n{}\n>>> APPROVAL REQUEST END\n";
    let searched = |source: Value| {
        let mut state = ParserState::default();
        state.parse(
            Harness::Codex,
            &json!({"type":"session_meta","payload":{"id":"s","source":source}}),
        );
        state
            .parse(
                Harness::Codex,
                &json!({"type":"response_item","payload":{"type":"message","role":"user",
                    "content":[{"type":"input_text","text":prompt}]}}),
            )
            .into_iter()
            .find(|fact| fact.kind == "message")
            .map(|fact| {
                (
                    llm_history_core::fact_search_text(&fact).to_owned(),
                    fact.text,
                )
            })
            .unwrap()
    };
    let (action, text) = searched(json!({"subagent":{"other":"guardian"}}));
    assert_eq!((action.as_str(), text.as_str()), ("{}", prompt));
    // Someone pasting a guardian prompt into a normal session still searches all of it.
    assert_eq!(searched(json!("cli")).0, prompt);
}

#[test]
fn exclusion_rules_follow_harness() {
    let text = "<environment_context> and <instructions> tags";
    let mut p = ParserState::default();
    let claude = &fixture_record(Harness::Claude, "s", text)[0];
    assert!(!p.parse(Harness::Claude, claude)[0].excluded);
    let mut meta = claude.clone();
    meta["isMeta"] = json!(true);
    assert!(p.parse(Harness::Claude, &meta)[0].excluded);
    let codex = &fixture_record(Harness::Codex, "s", text)[1];
    assert!(p.parse(Harness::Codex, codex)[0].excluded);
}
