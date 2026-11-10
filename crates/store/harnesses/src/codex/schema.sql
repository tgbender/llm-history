CREATE TABLE IF NOT EXISTS native_threads (profile_id VARCHAR,id VARCHAR,rollout_path VARCHAR,created_at VARCHAR,updated_at VARCHAR,source VARCHAR,cwd VARCHAR,title VARCHAR,archived BOOLEAN,first_user_message VARCHAR,model VARCHAR,reasoning_effort VARCHAR,thread_source VARCHAR,preview VARCHAR,source_db VARCHAR,name VARCHAR,agent_path VARCHAR,PRIMARY KEY(profile_id,id));

-- Session titles are one-line labels: the first non-blank line, at most 120
-- characters (llm_history_core::title_line). Codex keeps the whole first message
-- as a thread's title until it names the thread, so a title or preview equal to
-- the first message only counts after the generated name, a guardian review's
-- label and the subagent task path.
CREATE OR REPLACE MACRO title_line(text) AS (
    SELECT CASE WHEN length(line) > 120 THEN left(line, 119) || '…' ELSE line END
    FROM (SELECT regexp_extract(coalesce(text, ''), '^\s*(\S(?:[^\r\n]*\S)?)', 1) AS line)
);
CREATE OR REPLACE MACRO thread_title(name, title, preview, first_message, agent_path, source) AS
    title_line(coalesce(
        nullif(name, ''),
        CASE WHEN regexp_matches(coalesce(source, ''), '"other"\s*:\s*"guardian"') THEN 'Guardian review' END,
        nullif(nullif(title, ''), first_message),
        nullif(nullif(preview, ''), first_message),
        nullif(regexp_extract(coalesce(agent_path, ''), '[^/]+$'), ''),
        first_message
    ));
