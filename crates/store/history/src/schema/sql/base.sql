CREATE SEQUENCE IF NOT EXISTS entity_ids START 1000000000000;
CREATE TABLE IF NOT EXISTS meta (key VARCHAR PRIMARY KEY, value VARCHAR NOT NULL);
CREATE TABLE IF NOT EXISTS profiles (id VARCHAR PRIMARY KEY, name VARCHAR UNIQUE NOT NULL, data VARCHAR NOT NULL);
CREATE TABLE IF NOT EXISTS sources (id BIGINT PRIMARY KEY,profile_id VARCHAR NOT NULL,source_key VARCHAR NOT NULL,uri VARCHAR,committed UBIGINT,ordinal UBIGINT,prefix_hash VARCHAR,parser_state VARCHAR,generation BIGINT,missing BOOLEAN,parser_version INTEGER DEFAULT 1,observed_size UBIGINT,observed_mtime_ns BIGINT,head_xxh64 UBIGINT,boundary_xxh64 UBIGINT,UNIQUE(profile_id,source_key));
CREATE TABLE IF NOT EXISTS generations (id BIGINT PRIMARY KEY,source_id BIGINT,number BIGINT,reason VARCHAR,bytes UBIGINT,lines UBIGINT,prefix_hash VARCHAR,complete BOOLEAN,UNIQUE(source_id,number));
-- Large append-only tables carry no hash-keyed indexes: capture guarantees their
-- uniqueness, and DuckDB rewrites index blocks at every checkpoint.
CREATE TABLE IF NOT EXISTS native_documents (hash VARCHAR,raw_size UBIGINT,xxh64 UBIGINT,data BLOB);
CREATE TABLE IF NOT EXISTS native_observations (generation_id BIGINT,ordinal UBIGINT,start_byte UBIGINT,end_byte UBIGINT,payload_hash VARCHAR,raw_id BIGINT,json_valid BOOLEAN,capture_id BIGINT);
CREATE TABLE IF NOT EXISTS sessions (id BIGINT PRIMARY KEY,profile_id VARCHAR,native_id VARCHAR,cwd VARCHAR,title VARCHAR,model VARCHAR,parent_native VARCHAR,relation VARCHAR,subagent BOOLEAN,project VARCHAR,UNIQUE(profile_id,native_id));
-- One row per fact. Canonical, provider and message-attribute surfaces use these columns.
CREATE TABLE IF NOT EXISTS events (id BIGINT PRIMARY KEY,source_id BIGINT,session_id BIGINT,ordinal UBIGINT,fact_index BIGINT,payload_hash VARCHAR,kind VARCHAR,ts VARCHAR,role VARCHAR,phase VARCHAR,text VARCHAR,text_hash VARCHAR,model VARCHAR,tool VARCHAR,call_id VARCHAR,input_tokens BIGINT,output_tokens BIGINT,cache_read BIGINT,active BOOLEAN,cache_write BIGINT DEFAULT 0,excluded BOOLEAN DEFAULT false,usage_key VARCHAR DEFAULT '',effort VARCHAR DEFAULT '',thinking_tokens BIGINT DEFAULT 0,usage_eligible BOOLEAN DEFAULT true,generation_id BIGINT,raw_record_id BIGINT,session_seq BIGINT,context_epoch_id BIGINT,timestamp_utc TIMESTAMPTZ,timestamp_offset_minutes INTEGER,timestamp_timezone VARCHAR,timestamp_basis VARCHAR,turn_id VARCHAR,event_kind VARCHAR,native_event_type VARCHAR,native_payload_type VARCHAR,metadata_json VARCHAR,native_item_id VARCHAR,native_parent_id VARCHAR,native_turn_id VARCHAR,arguments_json VARCHAR,agent_protocol BOOLEAN DEFAULT false);
CREATE TABLE IF NOT EXISTS lexical_documents (id BIGINT PRIMARY KEY,source_id BIGINT,body VARCHAR,aliases VARCHAR);
CREATE TABLE IF NOT EXISTS model_profiles (id VARCHAR PRIMARY KEY,data VARCHAR);
CREATE TABLE IF NOT EXISTS embeddings (profile_id VARCHAR,message_id BIGINT,text_hash VARCHAR,chunk BIGINT,vector FLOAT[] NOT NULL,PRIMARY KEY(profile_id,message_id,chunk));
CREATE TABLE IF NOT EXISTS embedding_failures (profile_id VARCHAR,message_id BIGINT,text_hash VARCHAR,reason VARCHAR,PRIMARY KEY(profile_id,message_id));
CREATE OR REPLACE VIEW messages AS SELECT * FROM events WHERE active AND kind='message' AND text<>'';
CREATE OR REPLACE VIEW session_relations AS SELECT child.id source_session_id,parent.id target_session_id,child.relation FROM sessions child JOIN sessions parent ON parent.profile_id=child.profile_id AND parent.native_id=child.parent_native;
CREATE OR REPLACE VIEW history_dictionary AS SELECT table_name,column_name,data_type FROM information_schema.columns WHERE table_schema='main';
CREATE OR REPLACE VIEW history_help AS SELECT * FROM (VALUES ('messages','Active normalized messages; IDs are stable, source events retain raw provenance'),('sessions','Profile-scoped native sessions and metadata'),('session_relations','Parent/fork relationships resolved within the same profile'),('sources','Physical transcript identity and committed capture state'),('generations','Retained byte-exact source generations'),('profiles','Durable store identity and user-facing configuration')) entries(surface,description);

CREATE TABLE IF NOT EXISTS projects (id BIGINT PRIMARY KEY,profile_id VARCHAR,native_id VARCHAR,name VARCHAR,kind VARCHAR,metadata VARCHAR,active BOOLEAN,UNIQUE(profile_id,native_id));
CREATE TABLE IF NOT EXISTS project_roots (project_id BIGINT,path VARCHAR,ordinal INTEGER,PRIMARY KEY(project_id,path));
CREATE TABLE IF NOT EXISTS project_assignments (session_id BIGINT PRIMARY KEY,project_id BIGINT,evidence VARCHAR,snapshot_id BIGINT,basis_cwd VARCHAR DEFAULT '');

-- Message attributes derived from event columns, shared by views and queries.
CREATE OR REPLACE MACRO history_message_kind(kind, excluded, text) AS CASE WHEN kind IN ('tool_call','tool_result') THEN kind WHEN excluded AND (lower(ltrim(text)) LIKE '<environment_context>%' OR lower(ltrim(text)) LIKE '# agents.md%' OR lower(text) LIKE '%<instructions>%') THEN 'context' WHEN excluded THEN 'harness_notice' ELSE 'conversation' END;
CREATE OR REPLACE MACRO history_authorship(excluded, role, session_kind) AS CASE WHEN excluded THEN 'harness' WHEN role='assistant' OR (role='user' AND session_kind IN ('subagent','guardian')) THEN 'agent' WHEN role='user' THEN 'human' ELSE 'harness' END;
CREATE OR REPLACE MACRO history_authorship_evidence(excluded, role) AS CASE WHEN excluded THEN 'native_marker' WHEN role='user' THEN 'adapter_inferred' ELSE 'protocol_role' END;
CREATE OR REPLACE MACRO history_origin(excluded, role, session_kind) AS CASE WHEN excluded THEN 'harness_context' WHEN role='user' AND session_kind IN ('subagent','guardian') THEN 'delegation' WHEN role='user' THEN 'direct_input' WHEN role='assistant' THEN 'model_output' WHEN role='tool' THEN 'tool_payload' ELSE 'unknown' END;
