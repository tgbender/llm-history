CREATE TABLE IF NOT EXISTS provider_turn_contexts (
 event_id BIGINT PRIMARY KEY,cwd VARCHAR,current_date VARCHAR,timezone VARCHAR,model VARCHAR,effort VARCHAR,approval_policy VARCHAR,sandbox_policy VARCHAR,personality VARCHAR,service_tier VARCHAR,collaboration_mode_json VARCHAR,workspace_roots_json VARCHAR
);
CREATE TABLE IF NOT EXISTS provider_token_events (
 event_id BIGINT PRIMARY KEY,token_ordinal BIGINT,context_line_no BIGINT,
 total_input_tokens BIGINT,total_cached_input_tokens BIGINT,total_cache_write_input_tokens BIGINT,total_output_tokens BIGINT,total_reasoning_output_tokens BIGINT,total_tokens BIGINT,
 last_input_tokens BIGINT,last_cached_input_tokens BIGINT,last_cache_write_input_tokens BIGINT,last_output_tokens BIGINT,last_reasoning_output_tokens BIGINT,last_total_tokens BIGINT,
 model_context_window BIGINT,limit_id VARCHAR
);
CREATE OR REPLACE VIEW event_details AS
SELECT e.id event_id,e.session_id canonical_session_id,e.source_id,p.name profile,s.profile_id store_id,s.native_id thread_id,cs.native_runtime_id session_id,src.uri path,e.ordinal::BIGINT line_no,e.ts,e.timestamp_utc,e.timestamp_offset_minutes,e.timestamp_timezone,e.timestamp_basis,e.turn_id
FROM events e JOIN sessions s ON s.id=e.session_id JOIN canonical_sessions cs ON cs.id=s.id JOIN profiles p ON p.id=s.profile_id JOIN sources src ON src.id=e.source_id
WHERE e.fact_index=0 AND e.active AND e.session_seq IS NOT NULL;
CREATE OR REPLACE VIEW turn_contexts AS
SELECT d.*,c.* EXCLUDE(event_id) FROM provider_turn_contexts c JOIN event_details d ON d.event_id=c.event_id;
CREATE OR REPLACE VIEW token_events AS
SELECT d.*,t.* EXCLUDE(event_id),e.model,e.effort FROM provider_token_events t JOIN event_details d ON d.event_id=t.event_id JOIN events e ON e.id=t.event_id;
CREATE OR REPLACE VIEW normalized_events AS SELECT * FROM events WHERE active;

CREATE OR REPLACE VIEW messages AS SELECT * FROM normalized_events WHERE kind='message' AND text<>'';
