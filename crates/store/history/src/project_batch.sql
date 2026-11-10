CREATE TEMP TABLE resolved_projects AS
SELECT coalesce(p.id,nextval('entity_ids')) id,i.*
FROM temp.project_input i LEFT JOIN projects p USING (profile_id,native_id);

INSERT INTO projects
SELECT id,profile_id,native_id,name,kind,metadata,true FROM temp.resolved_projects
ON CONFLICT(id) DO UPDATE SET name=excluded.name,kind=excluded.kind,metadata=excluded.metadata,active=true
WHERE NOT projects.active OR projects.name IS DISTINCT FROM excluded.name
    OR projects.kind IS DISTINCT FROM excluded.kind OR projects.metadata IS DISTINCT FROM excluded.metadata;

DELETE FROM project_roots r USING temp.resolved_projects p
WHERE r.project_id=p.id AND NOT EXISTS (
    SELECT 1 FROM temp.project_root_input i WHERE i.native_id=p.native_id AND i.path=r.path
);

INSERT INTO project_roots
SELECT p.id,i.path,i.ordinal FROM temp.project_root_input i JOIN temp.resolved_projects p USING (native_id)
ON CONFLICT(project_id,path) DO UPDATE SET ordinal=excluded.ordinal
WHERE project_roots.ordinal IS DISTINCT FROM excluded.ordinal;

DROP TABLE temp.resolved_projects;
DROP TABLE temp.project_input;
DROP TABLE temp.project_root_input;
