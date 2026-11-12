CREATE TABLE IF NOT EXISTS provider_turn_contexts (
 event_id BIGINT PRIMARY KEY,cwd VARCHAR,current_date VARCHAR,timezone VARCHAR,model VARCHAR,effort VARCHAR,approval_policy VARCHAR,sandbox_policy VARCHAR,personality VARCHAR,service_tier VARCHAR,collaboration_mode_json VARCHAR,workspace_roots_json VARCHAR
);
CREATE TABLE IF NOT EXISTS provider_token_events (
 event_id BIGINT PRIMARY KEY,token_ordinal BIGINT,context_line_no BIGINT,
 total_input_tokens BIGINT,total_cached_input_tokens BIGINT,total_cache_write_input_tokens BIGINT,total_output_tokens BIGINT,total_reasoning_output_tokens BIGINT,total_tokens BIGINT,
 last_input_tokens BIGINT,last_cached_input_tokens BIGINT,last_cache_write_input_tokens BIGINT,last_output_tokens BIGINT,last_reasoning_output_tokens BIGINT,last_total_tokens BIGINT,
 model_context_window BIGINT,limit_id VARCHAR
);
