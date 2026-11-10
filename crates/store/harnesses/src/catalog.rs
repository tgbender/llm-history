//! Native catalog retention, composed atomically with shared project projections.
use anyhow::Result;
use duckdb::{Connection, OptionalExt, params};
use llm_history_store_engine::WriteTransaction;
use std::collections::BTreeSet;

pub struct Snapshot {
    pub id: i64,
    pub data: String,
}
pub struct SnapshotWrite<'a> {
    pub id: i64,
    pub profile: &'a str,
    pub uri: &'a str,
    pub format: &'a str,
    pub digest: &'a str,
    pub data: &'a str,
}
pub fn current(connection: &Connection, profile: &str) -> Result<Option<Snapshot>> {
    Ok(connection.query_row("select id,data from catalog_snapshots where profile_id=? and current order by id desc limit 1",[profile],|r|Ok(Snapshot{id:r.get(0)?,data:r.get(1)?})).optional()?)
}
pub fn write_snapshot(transaction: &WriteTransaction<'_>, row: SnapshotWrite<'_>) -> Result<()> {
    let connection = transaction.connection();
    connection.execute("insert into catalog_snapshots values (?,?,?,?,?,?,now(),true) on conflict(id) do update set uri=excluded.uri,format=excluded.format,digest=excluded.digest,data=excluded.data,captured_at=excluded.captured_at",params![row.id,row.profile,row.uri,row.format,row.digest,row.data])?;
    connection.execute(
        "delete from catalog_snapshots where profile_id=? and id<>?",
        params![row.profile, row.id],
    )?;
    Ok(())
}
pub fn reconcile_assignments(
    transaction: &WriteTransaction<'_>,
    profile: &str,
    next: &BTreeSet<(String, String)>,
) -> Result<BTreeSet<String>> {
    let connection = transaction.connection();
    connection.execute_batch(
        "CREATE TEMP TABLE next_native_assignments (profile_id VARCHAR, session_native VARCHAR, project_native VARCHAR)",
    )?;
    {
        let mut app =
            connection.appender_to_catalog_and_db("next_native_assignments", "temp", "main")?;
        for (session, project) in next {
            app.append_row(params![profile, session, project])?;
        }
        app.flush()?;
    }
    let changed = {
        let mut st = connection.prepare(
            "SELECT session_native FROM (
                SELECT a.session_native FROM native_project_assignments a
                ANTI JOIN temp.next_native_assignments n USING (profile_id,session_native,project_native)
                WHERE a.profile_id=?
                UNION
                SELECT n.session_native FROM temp.next_native_assignments n
                ANTI JOIN native_project_assignments a USING (profile_id,session_native,project_native)
             )",
        )?;
        st.query_map([profile], |r| r.get(0))?
            .collect::<duckdb::Result<_>>()?
    };
    connection.execute(
        "DELETE FROM native_project_assignments a WHERE a.profile_id=? AND NOT EXISTS (
            SELECT 1 FROM temp.next_native_assignments n WHERE n.profile_id=a.profile_id
            AND n.session_native=a.session_native AND n.project_native=a.project_native
         )",
        [profile],
    )?;
    connection.execute_batch(
        "INSERT INTO native_project_assignments SELECT n.* FROM temp.next_native_assignments n
         ANTI JOIN native_project_assignments a USING (profile_id,session_native,project_native);
         DROP TABLE temp.next_native_assignments;",
    )?;
    Ok(changed)
}
