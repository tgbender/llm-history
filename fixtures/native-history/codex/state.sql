-- Transcript-facing subset of state_5.sqlite at Codex 2bc8e7f5459b084afa482c7aac3f56fef36286b3.
PRAGMA foreign_keys = ON;
CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    metadata TEXT NOT NULL DEFAULT '{}',
    position INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE TABLE project_roots (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    path TEXT NOT NULL,
    PRIMARY KEY (project_id, position)
);
CREATE TABLE thread_sections (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL
, appearance TEXT);
CREATE TABLE threads (
    id TEXT PRIMARY KEY,
    rollout_path TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    source TEXT NOT NULL,
    model_provider TEXT NOT NULL,
    cwd TEXT NOT NULL,
    title TEXT NOT NULL,
    sandbox_policy TEXT NOT NULL,
    approval_mode TEXT NOT NULL,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    has_user_event INTEGER NOT NULL DEFAULT 0,
    archived INTEGER NOT NULL DEFAULT 0,
    archived_at INTEGER,
    git_sha TEXT,
    git_branch TEXT,
    git_origin_url TEXT
, cli_version TEXT NOT NULL DEFAULT '', first_user_message TEXT NOT NULL DEFAULT '', agent_nickname TEXT, agent_role TEXT, memory_mode TEXT NOT NULL DEFAULT 'enabled', model TEXT, reasoning_effort TEXT, agent_path TEXT, created_at_ms INTEGER, updated_at_ms INTEGER, thread_source TEXT, preview TEXT NOT NULL DEFAULT '', recency_at INTEGER NOT NULL DEFAULT 0, recency_at_ms INTEGER NOT NULL DEFAULT 0, history_mode TEXT NOT NULL DEFAULT 'legacy', name TEXT, is_pinned INTEGER NOT NULL DEFAULT 0, thread_section_id TEXT
    REFERENCES thread_sections(id) ON DELETE SET NULL, section_position INTEGER, section_entered_at_ms INTEGER, project_id TEXT
    REFERENCES projects(id) ON DELETE SET NULL, originator TEXT, daybreak_enabled BOOLEAN, creator_user_id TEXT, creator_account_id TEXT, attached_thread_parent_id TEXT);
INSERT INTO projects (id, name, position, created_at_ms, updated_at_ms) VALUES ('fixture-project', 'Fixture project', 0, 1794078000000, 1794078000000);
INSERT INTO project_roots (project_id, position, path) VALUES ('fixture-project', 0, '/fixture/project');
INSERT INTO thread_sections (id, name, appearance) VALUES ('fixture-section', 'Fixture section', '{"fixture":true}');
INSERT INTO threads (id, rollout_path, created_at, updated_at, source, model_provider, cwd, title, sandbox_policy, approval_mode, archived, history_mode, project_id, thread_section_id) VALUES ('00000000-0000-4000-8000-000000000001', '/fixture/codex/archived_sessions/rollout-2026-11-07T19-00-00-00000000-0000-4000-8000-000000000001.jsonl.zst', 1794078000, 1794078011, 'cli', 'openai', '/fixture/project', 'Fixture thread 1', '{"type":"read-only"}', 'never', 1, 'paginated', 'fixture-project', 'fixture-section');
INSERT INTO threads (id, rollout_path, created_at, updated_at, source, model_provider, cwd, title, sandbox_policy, approval_mode, archived, history_mode, project_id, thread_section_id) VALUES ('00000000-0000-4000-8000-000000000002', '/fixture/codex/sessions/2026/11/07/rollout-2026-11-07T19-00-09-00000000-0000-4000-8000-000000000002.jsonl', 1794078000, 1794078012, 'cli', 'openai', '/fixture/project', 'Fixture thread 2', '{"type":"read-only"}', 'never', 0, 'paginated', 'fixture-project', 'fixture-section');
INSERT INTO threads (id, rollout_path, created_at, updated_at, source, model_provider, cwd, title, sandbox_policy, approval_mode, archived, history_mode, project_id, thread_section_id) VALUES ('00000000-0000-4000-8000-000000000003', '/fixture/codex/sessions/2026/11/07/rollout-2026-11-07T19-00-20-00000000-0000-4000-8000-000000000003.jsonl', 1794078000, 1794078013, 'cli', 'openai', '/fixture/project', 'Fixture thread 3', '{"type":"read-only"}', 'never', 0, 'paginated', 'fixture-project', 'fixture-section');
INSERT INTO threads (id, rollout_path, created_at, updated_at, source, model_provider, cwd, title, sandbox_policy, approval_mode, archived, history_mode, project_id, thread_section_id) VALUES ('00000000-0000-4000-8000-000000000004', '/fixture/codex/sessions/2026/11/07/rollout-2026-11-07T19-00-21-00000000-0000-4000-8000-000000000004.jsonl', 1794078000, 1794078014, 'cli', 'openai', '/fixture/project', 'Fixture thread 4', '{"type":"read-only"}', 'never', 0, 'legacy', 'fixture-project', 'fixture-section');
