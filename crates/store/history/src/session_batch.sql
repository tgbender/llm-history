-- Preserve field precedence across ordered metadata changes, including a
-- nonempty title or canonical metadata followed by a sparse update.
CREATE OR REPLACE TEMP TABLE stage_session_latest AS
SELECT * FROM temp.stage_session_updates
QUALIFY row_number() OVER (PARTITION BY id ORDER BY pos DESC)=1;

INSERT INTO sessions
SELECT u.id,u.profile_id,u.native_id,u.cwd,coalesce(f.title,old.title,''),u.model,
       u.parent,u.relation,u.subagent,
       CASE WHEN f.cwd_count=1 AND u.cwd=old.cwd AND old.project<>'' THEN old.project ELSE u.project END
FROM temp.stage_session_latest u
JOIN (
    SELECT id,arg_max(nullif(title,''),pos) title,count(DISTINCT cwd) cwd_count
    FROM temp.stage_session_updates GROUP BY id
) f USING (id)
LEFT JOIN sessions old USING (id)
ON CONFLICT(id) DO UPDATE SET cwd=excluded.cwd,title=excluded.title,model=excluded.model,
    parent_native=excluded.parent_native,relation=excluded.relation,subagent=excluded.subagent,
    project=excluded.project;

INSERT INTO session_numbers(session_id)
SELECT u.id FROM temp.stage_session_latest u ANTI JOIN session_numbers n ON n.session_id=u.id
ORDER BY u.id;

INSERT INTO canonical_sessions
SELECT u.id,u.profile_id,u.harness,u.native_id,f.runtime,u.kind,u.native_kind,
       u.evidence,f.started,NULL,NULL,u.cwd,u.repository,u.model,u.label,f.metadata,true,now(),now()
FROM temp.stage_session_latest u
JOIN (
    SELECT id,arg_max(runtime,pos) runtime,arg_min(try_cast(started AS TIMESTAMPTZ),pos) started,arg_max(metadata,pos) metadata
    FROM temp.stage_session_updates GROUP BY id
) f USING (id)
ON CONFLICT(id) DO UPDATE SET native_runtime_id=coalesce(excluded.native_runtime_id,canonical_sessions.native_runtime_id),
    session_kind=excluded.session_kind,native_session_kind=excluded.native_session_kind,
    session_kind_evidence=excluded.session_kind_evidence,
    started_at_utc=coalesce(canonical_sessions.started_at_utc,excluded.started_at_utc),
    working_directory=excluded.working_directory,repository_root=excluded.repository_root,model=excluded.model,
    agent_label=excluded.agent_label,metadata_json=coalesce(excluded.metadata_json,canonical_sessions.metadata_json),
    active=true,updated_at=now();
