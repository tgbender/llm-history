use crate::Store;
use anyhow::Result;
use llm_history_types::Harness;
use llm_history_types::query::Filter;
use serde_json::json;

#[test]
fn colliding_thread_tool_and_parent_ids_remain_scoped_to_profiles() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(&directory.path().join("profiles.duckdb"), false)?;
    for name in ["personal", "work"] {
        let root = directory.path().join(name);
        std::fs::create_dir_all(root.join("sessions"))?;
        store.add_profile(name, Harness::Codex, &root, Some(name.into()), None)?;
        for (native, parent) in [("parent", ""), ("child", "parent")] {
            let records = [
                json!({"type":"session_meta","payload":{"id":native,"forked_from_id":parent,"cwd":format!("/projects/{name}")}}),
                json!({"type":"response_item","payload":{"type":"function_call","call_id":"same-tool-id","name":"Read","arguments":name}}),
                json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"same-tool-id","output":name}}),
                json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":name}}),
            ];
            std::fs::write(
                root.join("sessions").join(format!("{native}.jsonl")),
                records.iter().map(|r| format!("{r}\n")).collect::<String>(),
            )?;
        }
    }
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    let count: i64 = store.database.connection().query_row(
        "select count(*) \
         from session_relations r \
         join sessions c on c.id=r.source_session_id \
         join sessions p on p.id=r.target_session_id \
         where c.profile_id=p.profile_id",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(count, 2);
    for name in ["personal", "work"] {
        let messages = store.messages_page(
            &Filter {
                profiles: vec![name.into()],
                project: Some(name.into()),
                ..Filter::default()
            },
            None,
            usize::MAX,
        )?;
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|m| m.text == name && m.project == name));
        for message in messages {
            assert!(
                store
                    .context(message.message_no, 50, 50, &[name.into()])?
                    .iter()
                    .all(|m| m.profile == name)
            );
        }
    }
    assert_eq!(store.database.connection().query_row("select count(distinct session_id) from events where kind='tool_call' and call_id='same-tool-id'", [], |r| r.get::<_, i64>(0))?, 4);
    let mut profile = store.profiles(&["work".into()], false)?.remove(0);
    profile.enabled = false;
    store.save_profile(&profile)?;
    assert_eq!(
        store
            .messages_page(
                &Filter {
                    profiles: vec!["work".into()],
                    ..Filter::default()
                },
                None,
                usize::MAX
            )?
            .len(),
        2
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlink_root_aliases_cannot_register_the_same_store_twice() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("actual");
    std::fs::create_dir(&root)?;
    let alias = directory.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias)?;
    let store = Store::open(&directory.path().join("profiles.duckdb"), false)?;
    store.add_profile("personal", Harness::Codex, &root, None, None)?;
    assert!(
        store
            .add_profile("duplicate", Harness::Codex, &alias, None, None)
            .is_err()
    );
    Ok(())
}
