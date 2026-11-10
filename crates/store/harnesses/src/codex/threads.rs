//! Native Codex table writes borrow the history store's existing transaction.
use anyhow::Result;
use duckdb::params;
use llm_history_store_engine::WriteTransaction;
use llm_history_types::native::codex::ThreadMetadata;

pub fn write(
    transaction: &WriteTransaction<'_>,
    profile: &str,
    rows: &[ThreadMetadata],
) -> Result<usize> {
    if rows.is_empty() {
        return Ok(0);
    }
    let connection = transaction.connection();
    connection.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS stage_threads AS SELECT * FROM native_threads LIMIT 0",
    )?;
    let mut app = connection.appender_to_catalog_and_db("stage_threads", "temp", "main")?;
    for row in rows {
        app.append_row(params![
            profile,
            row.id,
            row.rollout_path,
            row.created_at,
            row.updated_at,
            row.source,
            row.cwd,
            row.title,
            row.archived,
            row.first_user_message,
            row.model,
            row.reasoning_effort,
            row.thread_source,
            row.preview,
            row.source_db,
            row.name,
            row.agent_path
        ])?;
    }
    drop(app);
    let fields = [
        "rollout_path",
        "created_at",
        "updated_at",
        "source",
        "cwd",
        "title",
        "archived",
        "first_user_message",
        "model",
        "reasoning_effort",
        "thread_source",
        "preview",
        "name",
        "agent_path",
        "source_db",
    ];
    let updates = fields
        .iter()
        .map(|c| format!("{c}=excluded.{c}"))
        .collect::<Vec<_>>()
        .join(",");
    let changed = fields
        .iter()
        .map(|c| format!("native_threads.{c} IS DISTINCT FROM excluded.{c}"))
        .collect::<Vec<_>>()
        .join(" OR ");
    connection.execute_batch(&format!("INSERT INTO native_threads SELECT * FROM temp.stage_threads ON CONFLICT(profile_id,id) DO UPDATE SET {updates} WHERE {changed}; DELETE FROM temp.stage_threads;"))?;
    Ok(rows.len())
}
