-- Lagging thread_history_1.sqlite projection cursor from the pinned Codex schema.
CREATE TABLE thread_history_projection_state (
    thread_id TEXT PRIMARY KEY,
    next_rollout_byte_offset INTEGER NOT NULL,
    next_rollout_ordinal INTEGER NOT NULL
);
INSERT INTO thread_history_projection_state VALUES ('00000000-0000-4000-8000-000000000001', 2133, 8);
