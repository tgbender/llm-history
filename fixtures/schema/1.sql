-- The 0.1.0 database layout (schema version 1), frozen from tag v0.1.0.
-- The migration test builds a database from this file and upgrades it.

-- crates/store/history/src/schema/sql/base.sql
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


-- crates/store/history/src/schema/sql/numbers.sql
CREATE SEQUENCE message_no_seq START 1;
CREATE SEQUENCE session_no_seq START 1;

CREATE TABLE message_numbers (
    message_no BIGINT PRIMARY KEY DEFAULT nextval('message_no_seq'),
    event_id BIGINT UNIQUE NOT NULL
);

CREATE TABLE session_numbers (
    session_no BIGINT PRIMARY KEY DEFAULT nextval('session_no_seq'),
    session_id BIGINT UNIQUE NOT NULL
);


-- crates/store/history/src/schema/sql/content.sql
-- A payload hash still identifies the original native document. Its compressed
-- bytes contain the full line or a shorter layout with null at each linked offset.
-- Append tables use staged anti-joins instead of maintaining large hash indexes.
CREATE TABLE IF NOT EXISTS content_objects (id BLOB NOT NULL, byte_length UBIGINT NOT NULL, data BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS content_parts (document_hash VARCHAR NOT NULL, layout_offset UBIGINT NOT NULL, native_path VARCHAR NOT NULL, content_id BLOB NOT NULL);

CREATE SEQUENCE IF NOT EXISTS capture_ids;
CREATE TABLE IF NOT EXISTS captures (
    id BIGINT PRIMARY KEY,
    source_id BIGINT NOT NULL,
    generation_id BIGINT NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    location_uri VARCHAR NOT NULL,
    encoding VARCHAR NOT NULL,
    physical_cutoff UBIGINT NOT NULL,
    start_ordinal UBIGINT NOT NULL,
    start_decoded_byte UBIGINT NOT NULL,
    end_ordinal UBIGINT NOT NULL,
    end_decoded_byte UBIGINT NOT NULL,
    complete BOOLEAN NOT NULL
);


-- crates/store/harnesses/src/codex/schema.sql
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


-- crates/store/harnesses/src/catalog.sql
CREATE TABLE IF NOT EXISTS native_project_assignments (profile_id VARCHAR,session_native VARCHAR,project_native VARCHAR,PRIMARY KEY(profile_id,session_native,project_native));
CREATE TABLE IF NOT EXISTS catalog_snapshots (id BIGINT PRIMARY KEY,profile_id VARCHAR,uri VARCHAR,format VARCHAR,digest VARCHAR,data VARCHAR,captured_at TIMESTAMPTZ,current BOOLEAN);


-- crates/store/history/src/schema/sql/canonical.sql
-- Canonical conversation model, corresponding to Python schema 10.
create table if not exists canonical_sessions (
    id bigint primary key,
    store_id varchar not null,
    harness_key varchar not null,
    native_session_id varchar not null,
    native_runtime_id varchar,
    session_kind varchar not null,
    native_session_kind varchar,
    session_kind_evidence varchar not null,
    started_at_utc timestamptz,
    ended_at_utc timestamptz,
    last_event_at_utc timestamptz,
    working_directory varchar,
    repository_root varchar,
    model varchar,
    agent_label varchar,
    metadata_json varchar,
    active boolean not null,
    created_at timestamptz not null,
    updated_at timestamptz not null,
    unique(store_id, harness_key, native_session_id)
);

create table if not exists canonical_session_sources (
    session_id bigint not null,
    source_id bigint not null,
    current_generation_id bigint not null,
    first_seen_at timestamptz not null,
    last_seen_at timestamptz not null,
    active boolean not null,
    sequence_base bigint default 0,
    -- The session starts at the earliest active file's start.
    started_at_utc timestamptz,
    primary key(session_id, source_id)
);

create table if not exists canonical_session_relations (
    id bigint primary key,
    source_session_id bigint not null,
    target_session_id bigint,
    target_native_session_id varchar not null,
    relation_kind varchar not null,
    native_relation_kind varchar,
    evidence_kind varchar not null,
    evidence_raw_record_id bigint,
    metadata_json varchar,
    active boolean not null,
    created_at timestamptz not null,
    updated_at timestamptz not null,
    unique(source_session_id, relation_kind, target_native_session_id)
);

create table if not exists canonical_projects (
    id bigint primary key,
    source_id bigint,
    store_id varchar not null,
    harness_key varchar not null,
    native_project_id varchar not null,
    project_kind varchar not null,
    native_name varchar,
    display_name varchar not null,
    name_evidence varchar not null,
    metadata_json varchar,
    active boolean not null,
    created_at timestamptz not null,
    updated_at timestamptz not null,
    unique(store_id, harness_key, native_project_id)
);

create table if not exists canonical_project_roots (
    project_id bigint not null,
    root_path varchar not null,
    root_ordinal integer not null,
    root_kind varchar not null,
    evidence_kind varchar not null,
    active boolean not null,
    created_at timestamptz not null,
    updated_at timestamptz not null,
    primary key(project_id, root_path)
);

create table if not exists canonical_session_projects (
    session_id bigint not null,
    project_id bigint not null,
    source_id bigint,
    evidence_kind varchar not null,
    basis_working_directory varchar,
    active boolean not null,
    created_at timestamptz not null,
    updated_at timestamptz not null,
    primary key(session_id, project_id)
);

create index if not exists canonical_projects_name_idx on canonical_projects(display_name);

create index if not exists canonical_project_roots_path_idx on canonical_project_roots(root_path);

create index if not exists canonical_session_projects_session_idx on canonical_session_projects(session_id);

create table if not exists canonical_conversation_groups (
    id bigint primary key,
    root_session_id bigint not null unique,
    resolver_version varchar not null,
    active boolean not null,
    created_at timestamptz not null,
    updated_at timestamptz not null
);

create table if not exists canonical_conversation_memberships (
    conversation_group_id bigint not null,
    session_id bigint not null,
    membership_kind varchar not null,
    evidence_kind varchar not null,
    is_primary boolean not null,
    updated_at timestamptz not null,
    primary key(conversation_group_id, session_id)
);

create table if not exists canonical_context_epochs (
    id bigint primary key,
    session_id bigint not null,
    ordinal bigint not null,
    starts_at_event_id bigint,
    ends_at_event_id bigint,
    trigger_kind varchar not null,
    evidence_kind varchar not null,
    active boolean not null,
    created_at timestamptz not null,
    updated_at timestamptz not null,
    unique(session_id, ordinal)
);

create table if not exists canonical_event_relations (
    id bigint primary key,
    session_id bigint not null,
    source_id bigint not null,
    child_event_id bigint not null,
    parent_event_id bigint not null,
    child_source_ordinal bigint not null,
    parent_source_ordinal bigint not null,
    relation_kind varchar not null,
    native_relation_kind varchar,
    evidence_kind varchar not null,
    metadata_json varchar,
    active boolean not null,
    created_at timestamptz not null,
    updated_at timestamptz not null,
    unique(source_id, child_source_ordinal, relation_kind, parent_source_ordinal)
);

create index if not exists canonical_event_relations_child_idx on canonical_event_relations(child_event_id);

create index if not exists canonical_relations_target_idx on canonical_session_relations(target_session_id);


-- crates/store/history/src/schema/sql/event_views.sql
-- Canonical and provider surfaces over the single fact table. A fact with
-- fact_index 0 is the physical record; higher indexes are its typed tool blocks.
CREATE OR REPLACE VIEW canonical_events AS
SELECT id,session_id,source_id,generation_id,raw_record_id,ordinal::BIGINT AS source_ordinal,session_seq,context_epoch_id,timestamp_utc,event_kind,native_event_type,native_payload_type,metadata_json,active,native_item_id,native_parent_id,native_turn_id
FROM events WHERE fact_index=0 AND active AND session_seq IS NOT NULL;

CREATE OR REPLACE VIEW canonical_event_identities AS
SELECT source_id,ordinal::BIGINT AS source_ordinal,payload_hash AS payload_id,id AS event_id,generation_id AS first_generation_id,CAST(NULL AS TIMESTAMPTZ) AS created_at
FROM events WHERE fact_index=0;

CREATE OR REPLACE VIEW provider_event_details AS
SELECT id AS event_id,ts AS raw_timestamp,timestamp_utc,timestamp_offset_minutes,timestamp_timezone,timestamp_basis,turn_id
FROM events WHERE fact_index=0 AND active;

CREATE OR REPLACE VIEW canonical_messages AS
SELECT e.id AS event_id,e.session_id,e.session_seq,e.context_epoch_id,e.timestamp_utc,e.role AS protocol_role,
 history_message_kind(e.kind,e.excluded,e.text) AS message_kind,nullif(e.phase,'') AS phase,
 history_authorship(e.excluded,e.role,s.session_kind) AS authorship,history_authorship_evidence(e.excluded,e.role) AS authorship_evidence,
 history_origin(e.excluded,e.role,s.session_kind) AS origin,'original' AS record_relation,e.text,from_hex(e.text_hash) AS content_blake3,length(e.text) AS chars,coalesce(e.agent_protocol,false) AS agent_protocol
FROM events e JOIN canonical_sessions s ON s.id=e.session_id
WHERE e.fact_index=0 AND e.active AND e.session_seq IS NOT NULL AND e.kind='message' AND e.role<>'' AND e.text<>'';

CREATE OR REPLACE VIEW canonical_message_attributes AS
SELECT * REPLACE('' AS text) FROM canonical_messages;

CREATE OR REPLACE VIEW canonical_tool_calls AS
SELECT r.id AS event_id,row_number() OVER (PARTITION BY r.id ORDER BY f.fact_index)-1 AS emitted_ordinal,r.session_id,r.session_seq,r.timestamp_utc,
 f.call_id AS native_call_id,f.tool AS tool_name,coalesce(f.arguments_json,f.text) AS arguments_json,f.id AS legacy_event_id
FROM events f JOIN events r ON r.source_id=f.source_id AND r.ordinal=f.ordinal AND r.fact_index=0 AND r.active AND r.session_seq IS NOT NULL
WHERE f.active AND f.kind='tool_call';

CREATE OR REPLACE VIEW canonical_tool_results AS
SELECT r.id AS event_id,row_number() OVER (PARTITION BY r.id ORDER BY f.fact_index)-1 AS emitted_ordinal,r.session_id,r.session_seq,r.timestamp_utc,
 f.call_id AS native_call_id,length(f.text) AS output_chars,f.text_hash AS output_hash,left(f.text,500) AS output_preview,f.id AS legacy_event_id
FROM events f JOIN events r ON r.source_id=f.source_id AND r.ordinal=f.ordinal AND r.fact_index=0 AND r.active AND r.session_seq IS NOT NULL
WHERE f.active AND f.kind='tool_result';

CREATE OR REPLACE VIEW canonical_compactions AS
SELECT id AS event_id,session_id,context_epoch_id,text AS summary_text,'adapter_derived' AS evidence_kind
FROM events WHERE fact_index=0 AND active AND session_seq IS NOT NULL AND event_kind='compaction';


-- crates/store/history/src/schema/sql/provider.sql
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


-- crates/store/history/src/schema/sql/views.sql
-- History SQL surfaces corresponding to Python schema 10.
create or replace view canonical_session_ancestry as
with recursive ancestry(session_id, ancestor_session_id, depth, path) as (
    select id, id, 0::bigint, [id]::bigint[]
    from canonical_sessions
    where active
    union all
    select
        ancestry.session_id,
        relations.target_session_id,
        ancestry.depth + 1,
        list_append(ancestry.path, relations.target_session_id)
    from ancestry
    join canonical_session_relations relations
      on relations.source_session_id = ancestry.ancestor_session_id
     and relations.active
     and relations.target_session_id is not null
    join canonical_sessions targets
      on targets.id = relations.target_session_id and targets.active
    where not list_contains(ancestry.path, relations.target_session_id)
)
select session_id, ancestor_session_id, depth, path
from ancestry;

create or replace view canonical_timeline as
select
    events.id as event_id,
    events.session_id,
    events.session_seq,
    events.source_ordinal,
    events.context_epoch_id,
    events.timestamp_utc,
    events.event_kind,
    messages.protocol_role,
    messages.authorship,
    messages.origin,
    messages.phase,
    messages.text,
    calls.tool_name,
    calls.native_call_id,
    results.output_preview
from canonical_events events
left join canonical_messages messages on messages.event_id = events.id
left join canonical_tool_calls calls
  on calls.event_id = events.id and calls.emitted_ordinal = 0
left join canonical_tool_results results
  on results.event_id = events.id and results.emitted_ordinal = 0;

create or replace view canonical_search_documents as
select
    messages.event_id as document_id,
    messages.event_id,
    messages.session_id,
    memberships.conversation_group_id,
    sessions.harness_key,
    sessions.session_kind,
    sessions.working_directory,
    sessions.repository_root,
    messages.context_epoch_id,
    messages.session_seq,
    messages.timestamp_utc,
    messages.protocol_role,
    messages.message_kind,
    messages.phase,
    messages.authorship,
    messages.origin,
    messages.text,
    messages.content_blake3
from canonical_messages messages
join canonical_sessions sessions on sessions.id = messages.session_id and sessions.active
left join canonical_conversation_memberships memberships
  on memberships.session_id = messages.session_id and memberships.is_primary
where messages.message_kind = 'conversation'
  and messages.protocol_role in ('user', 'assistant');

create or replace view history_session_relations as
select
    relations.id as relation_id,
    relations.relation_kind,
    relations.native_relation_kind,
    relations.evidence_kind,
    relations.metadata_json,
    relations.active,
    children.id as child_session_id,
    children.native_session_id as child_native_session_id,
    children.harness_key,
    relations.target_session_id as parent_session_id,
    coalesce(parents.native_session_id, relations.target_native_session_id)
        as parent_native_session_id,
    relations.target_session_id is not null as is_resolved
from canonical_session_relations relations
join canonical_sessions children on children.id = relations.source_session_id
left join canonical_sessions parents on parents.id = relations.target_session_id;

create or replace view history_projects as
with roots_aggregated as (
    select
        project_id,
        list(root_path order by root_ordinal) filter (where active) as root_paths
    from canonical_project_roots
    group by project_id
), sessions_aggregated as (
    select project_id, count(distinct session_id) filter (where active) as session_count
    from canonical_session_projects
    group by project_id
)
select
    projects.id as project_id,
    projects.harness_key,
    projects.native_project_id,
    projects.project_kind,
    projects.native_name,
    projects.display_name as project_name,
    projects.name_evidence,
    projects.active,
    roots.root_paths,
    coalesce(memberships.session_count, 0) as session_count,
    projects.created_at,
    projects.updated_at
from canonical_projects projects
left join roots_aggregated roots on roots.project_id = projects.id
left join sessions_aggregated memberships on memberships.project_id = projects.id;

create or replace view history_sessions as
with primary_relations as (
    select * exclude(relation_rank)
    from (
        select
            relations.*,
            row_number() over (
                partition by relations.child_session_id
                order by
                    case relations.relation_kind
                        when 'spawned' then 0
                        when 'forked_from' then 1
                        when 'continued_from' then 2
                        else 3
                    end,
                    relations.relation_id
            ) as relation_rank
        from history_session_relations relations
        where relations.active
    )
    where relation_rank = 1
), primary_projects as (
    select * exclude(project_rank)
    from (
        select
            memberships.session_id,
            projects.id as project_id,
            projects.native_project_id,
            projects.project_kind,
            projects.display_name as project_name,
            memberships.evidence_kind as project_evidence,
            roots.root_path as project_root,
            row_number() over (
                partition by memberships.session_id
                order by
                    case memberships.evidence_kind
                        when 'native_explicit' then 0
                        when 'native_project_path' then 1
                        when 'root_path_match' then 2
                        when 'cwd_fallback' then 3
                        else 4
                    end,
                    case
                        when sessions.working_directory = roots.root_path
                          or starts_with(sessions.working_directory, roots.root_path || '/')
                          or starts_with(sessions.working_directory, roots.root_path || '\')
                        then 0 else 1
                    end,
                    length(roots.root_path) desc nulls last,
                    roots.root_ordinal nulls last,
                    projects.id
            ) as project_rank
        from canonical_session_projects memberships
        join canonical_projects projects on projects.id = memberships.project_id and projects.active
        join canonical_sessions sessions on sessions.id = memberships.session_id
        left join canonical_project_roots roots
          on roots.project_id = projects.id and roots.active
        where memberships.active
    )
    where project_rank = 1
), message_counts as (
    select session_id, count(*) as message_count
    from canonical_messages
    group by session_id
), tool_counts as (
    select session_id, count(*) as tool_call_count
    from canonical_tool_calls
    group by session_id
), compaction_counts as (
    select session_id, count(*) as compaction_count
    from canonical_compactions
    group by session_id
), child_counts as (
    select parent_session_id, count(*) as child_session_count
    from history_session_relations
    where active and is_resolved
    group by parent_session_id
)
select
    sessions.id as session_id,
    memberships.conversation_group_id,
    sessions.harness_key,
    sessions.native_session_id,
    sessions.native_runtime_id,
    sessions.session_kind,
    sessions.native_session_kind,
    sessions.session_kind_evidence,
    sessions.started_at_utc,
    sessions.ended_at_utc,
    sessions.last_event_at_utc,
    sessions.working_directory,
    sessions.repository_root,
    primary_projects.project_id,
    primary_projects.native_project_id,
    primary_projects.project_kind,
    primary_projects.project_name,
    primary_projects.project_root,
    primary_projects.project_evidence,
    sessions.model,
    sessions.agent_label,
    primary_relations.relation_kind as parent_relation,
    primary_relations.parent_session_id,
    primary_relations.parent_native_session_id,
    primary_relations.evidence_kind as parent_evidence,
    coalesce(message_counts.message_count, 0) as message_count,
    coalesce(tool_counts.tool_call_count, 0) as tool_call_count,
    coalesce(compaction_counts.compaction_count, 0) as compaction_count,
    coalesce(child_counts.child_session_count, 0) as child_session_count,
    numbers.session_no
from canonical_sessions sessions
left join session_numbers numbers on numbers.session_id = sessions.id
left join canonical_conversation_memberships memberships
  on memberships.session_id = sessions.id and memberships.is_primary
left join primary_relations on primary_relations.child_session_id = sessions.id
left join primary_projects on primary_projects.session_id = sessions.id
left join message_counts on message_counts.session_id = sessions.id
left join tool_counts on tool_counts.session_id = sessions.id
left join compaction_counts on compaction_counts.session_id = sessions.id
left join child_counts on child_counts.parent_session_id = sessions.id
where sessions.active;

create or replace view history_messages as
select
    messages.event_id as message_id,
    messages.event_id,
    messages.session_id,
    memberships.conversation_group_id,
    sessions.harness_key,
    sessions.native_session_id,
    sessions.session_kind,
    sessions.native_session_kind,
    sessions.session_kind_evidence,
    sessions.model,
    sessions.agent_label,
    sessions.parent_relation,
    sessions.parent_session_id,
    sessions.parent_native_session_id,
    sessions.working_directory,
    sessions.repository_root,
    sessions.project_id,
    sessions.native_project_id,
    sessions.project_kind,
    sessions.project_name,
    sessions.project_root,
    sessions.project_evidence,
    messages.session_seq,
    row_number() over (
        partition by messages.session_id order by messages.session_seq
    ) as message_ordinal,
    messages.context_epoch_id,
    epochs.ordinal as context_epoch_ordinal,
    messages.timestamp_utc,
    messages.protocol_role as role,
    messages.authorship,
    messages.authorship_evidence,
    messages.origin,
    messages.message_kind,
    messages.phase,
    messages.protocol_role = 'assistant'
        and messages.phase = 'final_answer' as is_final_answer,
    messages.message_kind = 'conversation'
        and messages.protocol_role in ('user', 'assistant') as is_searchable,
    messages.text,
    messages.chars,
    hex(messages.content_blake3) as content_blake3_hex,
    numbers.message_no,
    sessions.session_no
from canonical_messages messages
join history_sessions sessions on sessions.session_id = messages.session_id
left join message_numbers numbers on numbers.event_id = messages.event_id
left join canonical_context_epochs epochs on epochs.id = messages.context_epoch_id
left join canonical_conversation_memberships memberships
  on memberships.session_id = messages.session_id and memberships.is_primary;

create or replace view history_turn_pairs as
with ordered_messages as (
    select
        messages.*,
        sum(case when messages.protocol_role = 'user' then 1 else 0 end) over (
            partition by messages.session_id
            order by messages.session_seq, messages.event_id
            rows between unbounded preceding and current row
        )::bigint as turn_ordinal
    from canonical_messages messages
    join canonical_sessions sessions on sessions.id = messages.session_id and sessions.active
    where messages.message_kind = 'conversation'
      and messages.protocol_role in ('user', 'assistant')
), questions as (
    select *
    from ordered_messages
    where protocol_role = 'user'
), ranked_answers as (
    select
        answers.*,
        row_number() over (
            partition by answers.session_id, answers.turn_ordinal
            order by answers.session_seq, answers.event_id
        ) as answer_rank
    from ordered_messages answers
    where answers.protocol_role = 'assistant'
      and answers.phase = 'final_answer'
      and answers.turn_ordinal > 0
)
select
    questions.session_id,
    memberships.conversation_group_id,
    sessions.harness_key,
    sessions.native_session_id,
    sessions.session_kind,
    questions.turn_ordinal,
    questions.event_id as question_message_id,
    answers.event_id as answer_message_id,
    questions.session_seq as question_session_seq,
    answers.session_seq as answer_session_seq,
    questions.context_epoch_id as question_context_epoch_id,
    answers.context_epoch_id as answer_context_epoch_id,
    questions.timestamp_utc as question_timestamp_utc,
    answers.timestamp_utc as answer_timestamp_utc,
    questions.authorship as question_authorship,
    answers.authorship as answer_authorship,
    questions.origin as question_origin,
    answers.origin as answer_origin,
    answers.event_id is not null as is_answered
from questions
join canonical_sessions sessions on sessions.id = questions.session_id and sessions.active
left join ranked_answers answers
  on answers.session_id = questions.session_id
 and answers.turn_ordinal = questions.turn_ordinal
 and answers.answer_rank = 1
left join canonical_conversation_memberships memberships
  on memberships.session_id = questions.session_id and memberships.is_primary;

create or replace view history_timeline as
select
    timeline.event_id,
    timeline.session_id,
    memberships.conversation_group_id,
    sessions.harness_key,
    sessions.native_session_id,
    sessions.session_kind,
    sessions.native_session_kind,
    sessions.session_kind_evidence,
    sessions.model,
    sessions.agent_label,
    sessions.parent_relation,
    sessions.parent_session_id,
    sessions.parent_native_session_id,
    timeline.session_seq,
    timeline.source_ordinal,
    epochs.ordinal as context_epoch_ordinal,
    timeline.timestamp_utc,
    timeline.event_kind,
    timeline.protocol_role as role,
    timeline.authorship,
    timeline.origin,
    timeline.phase,
    timeline.tool_name,
    timeline.native_call_id,
    timeline.text,
    timeline.output_preview,
    coalesce(
        nullif(timeline.text, ''),
        nullif(compactions.summary_text, ''),
        nullif(timeline.output_preview, ''),
        case
            when timeline.tool_name is not null then 'tool: ' || timeline.tool_name
            else timeline.event_kind
        end
    ) as display_text
from canonical_timeline timeline
join history_sessions sessions on sessions.session_id = timeline.session_id
left join canonical_context_epochs epochs on epochs.id = timeline.context_epoch_id
left join canonical_compactions compactions on compactions.event_id = timeline.event_id
left join canonical_conversation_memberships memberships
  on memberships.session_id = timeline.session_id and memberships.is_primary;

create or replace view history_event_relations as
select
    relations.id as event_relation_id,
    relations.session_id,
    children.conversation_group_id,
    children.harness_key,
    relations.relation_kind,
    relations.native_relation_kind,
    relations.evidence_kind,
    relations.child_event_id,
    children.session_seq as child_session_seq,
    children.event_kind as child_event_kind,
    relations.parent_event_id,
    parents.session_seq as parent_session_seq,
    parents.event_kind as parent_event_kind,
    relations.child_source_ordinal,
    relations.parent_source_ordinal,
    relations.metadata_json
from canonical_event_relations relations
join history_timeline children on children.event_id = relations.child_event_id
join history_timeline parents on parents.event_id = relations.parent_event_id
where relations.active;

create or replace view history_tool_activity as
with paired as (
    select
        calls.event_id as call_event_id,
        calls.emitted_ordinal as call_ordinal,
        calls.session_id,
        calls.session_seq as call_session_seq,
        calls.timestamp_utc as called_at_utc,
        calls.native_call_id,
        calls.tool_name,
        calls.arguments_json,
        results.event_id as result_event_id,
        result_events.session_seq as result_session_seq,
        results.timestamp_utc as completed_at_utc,
        results.output_chars,
        results.output_hash,
        results.output_preview,
        row_number() over (
            partition by calls.event_id, calls.emitted_ordinal
            order by
                case when results.event_id is null then 1 else 0 end,
                case when result_events.session_seq >= calls.session_seq then 0 else 1 end,
                abs(coalesce(result_events.session_seq, calls.session_seq) - calls.session_seq)
        ) as result_rank
    from canonical_tool_calls calls
    left join canonical_tool_results results
      on results.session_id = calls.session_id
     and results.native_call_id = calls.native_call_id
     and calls.native_call_id is not null
    left join canonical_events result_events on result_events.id = results.event_id
)
select
    paired.call_event_id,
    paired.call_ordinal,
    paired.result_event_id,
    paired.session_id,
    memberships.conversation_group_id,
    sessions.harness_key,
    sessions.native_session_id,
    sessions.session_kind,
    sessions.working_directory,
    sessions.repository_root,
    paired.call_session_seq,
    paired.result_session_seq,
    paired.called_at_utc,
    paired.completed_at_utc,
    paired.native_call_id,
    paired.tool_name,
    paired.arguments_json,
    paired.output_chars,
    paired.output_hash,
    paired.output_preview,
    paired.result_event_id is not null as has_result
from paired
join canonical_sessions sessions on sessions.id = paired.session_id and sessions.active
left join canonical_conversation_memberships memberships
  on memberships.session_id = paired.session_id and memberships.is_primary
where paired.result_rank = 1;

create or replace view history_compactions as
select
    compactions.event_id,
    compactions.session_id,
    memberships.conversation_group_id,
    sessions.harness_key,
    sessions.native_session_id,
    sessions.session_kind,
    events.session_seq,
    events.timestamp_utc,
    epochs.ordinal as context_epoch_ordinal,
    compactions.summary_text,
    compactions.evidence_kind
from canonical_compactions compactions
join canonical_events events on events.id = compactions.event_id
join canonical_sessions sessions on sessions.id = compactions.session_id and sessions.active
left join canonical_context_epochs epochs on epochs.id = compactions.context_epoch_id
left join canonical_conversation_memberships memberships
  on memberships.session_id = compactions.session_id and memberships.is_primary;

create or replace view history_context_epochs as
select
    epochs.id as context_epoch_id,
    epochs.session_id,
    memberships.conversation_group_id,
    sessions.harness_key,
    sessions.native_session_id,
    sessions.session_kind,
    epochs.ordinal as context_epoch_ordinal,
    epochs.trigger_kind,
    epochs.evidence_kind,
    start_events.session_seq as starts_at_session_seq,
    start_events.timestamp_utc as starts_at_utc,
    end_events.session_seq as ends_at_session_seq,
    end_events.timestamp_utc as ends_at_utc
from canonical_context_epochs epochs
join canonical_sessions sessions on sessions.id = epochs.session_id and sessions.active
left join canonical_events start_events on start_events.id = epochs.starts_at_event_id
left join canonical_events end_events on end_events.id = epochs.ends_at_event_id
left join canonical_conversation_memberships memberships
  on memberships.session_id = epochs.session_id and memberships.is_primary
where epochs.active;

create or replace macro history_messages_for_session(p_session_id) as table (
    select * from history_messages
    where session_id = p_session_id
    order by session_seq
);

create or replace macro history_messages_for_conversation(p_conversation_group_id) as table (
    select * from history_messages
    where conversation_group_id = p_conversation_group_id
    order by timestamp_utc nulls last, session_id, session_seq
);

create or replace macro history_descendant_sessions(p_root_session_id) as table (
    select sessions.*, ancestry.depth as lineage_depth
    from canonical_session_ancestry ancestry
    join history_sessions sessions on sessions.session_id = ancestry.session_id
    where ancestry.ancestor_session_id = p_root_session_id
    order by ancestry.depth, sessions.started_at_utc nulls last, sessions.session_id
);

create or replace macro history_message_context(p_message_id, p_before, p_after) as table (
    with target as (
        select session_id, message_ordinal
        from history_messages
        where message_id = p_message_id
    )
    select
        messages.*,
        messages.message_id = p_message_id as is_target,
        messages.message_ordinal - target.message_ordinal as relative_message_offset
    from history_messages messages
    join target using(session_id)
    where messages.message_ordinal between
            greatest(target.message_ordinal - greatest(p_before, 0), 1)
        and target.message_ordinal + greatest(p_after, 0)
    order by messages.message_ordinal
);

create or replace macro history_event_context(p_event_id, p_before, p_after) as table (
    with target_session as (
        select session_id
        from history_timeline
        where event_id = p_event_id
    ), ordered as (
        select
            timeline.*,
            row_number() over (
                partition by timeline.session_id
                order by timeline.session_seq, timeline.event_id
            ) as event_ordinal
        from history_timeline timeline
        join target_session using(session_id)
    ), target as (
        select session_id, event_ordinal
        from ordered
        where event_id = p_event_id
    )
    select
        events.*,
        events.event_id = p_event_id as is_target,
        events.event_ordinal - target.event_ordinal as relative_event_offset
    from ordered events
    join target using(session_id)
    where events.event_ordinal between
            greatest(target.event_ordinal - greatest(p_before, 0), 1)
        and target.event_ordinal + greatest(p_after, 0)
    order by events.event_ordinal
);

create or replace macro history_event_branch(p_leaf_event_id) as table (
    with recursive ancestry(event_id, distance_from_leaf, path) as (
        select p_leaf_event_id::bigint, 0::bigint, [p_leaf_event_id::bigint]::bigint[]
        union all
        select
            relations.parent_event_id,
            ancestry.distance_from_leaf + 1,
            list_append(ancestry.path, relations.parent_event_id)
        from ancestry
        join history_event_relations relations
          on relations.child_event_id = ancestry.event_id
        where not list_contains(ancestry.path, relations.parent_event_id)
    )
    select
        timeline.*,
        ancestry.distance_from_leaf,
        row_number() over (
            order by ancestry.distance_from_leaf desc, timeline.event_id
        ) as branch_ordinal
    from ancestry
    join history_timeline timeline using(event_id)
    order by branch_ordinal
);

create or replace view history_dictionary as
select
    columns.table_name as surface,
    columns.ordinal_position,
    columns.column_name,
    columns.data_type,
    case
        when columns.column_name in ('session_id', 'message_id', 'event_id', 'relation_id',
                                     'conversation_group_id', 'context_epoch_id')
            then 'Stable BIGINT join key generated by llm-history.'
        when columns.column_name in ('question_message_id', 'answer_message_id')
            then 'Stable BIGINT key joining to history_messages.message_id; answer may be null.'
        when columns.column_name in ('native_session_id', 'native_runtime_id', 'native_call_id')
            then 'Provider-native identifier for session and tool correlation.'
        when columns.column_name = 'source_ordinal'
            then 'One-based event position in the exact captured source stream.'
        when columns.column_name = 'harness_key'
            then 'Registered source-adapter key for the originating harness.'
        when columns.column_name = 'session_kind'
            then 'Normalized kind: interactive, subagent, automation, guardian, or unknown.'
        when columns.column_name = 'native_session_kind'
            then 'Exact provider classification retained separately from the normalized kind.'
        when columns.column_name = 'session_kind_evidence'
            then 'Why the adapter assigned the normalized session kind.'
        when columns.column_name = 'role'
            then 'Protocol role; authorship separately records who actually produced the message.'
        when columns.column_name = 'authorship'
            then 'Normalized author identity; use common_values to discover observed values.'
        when columns.column_name = 'agent_label'
            then 'Provider-supplied agent label for attributing multi-agent conversation rows.'
        when columns.column_name = 'parent_relation'
            then 'Primary normalized relationship from this session to its parent session.'
        when columns.column_name = 'phase'
            then 'Message/event phase, including final_answer when the provider exposes it.'
        when columns.column_name = 'text'
            then 'Complete normalized message or event text; potentially large and sensitive.'
        when columns.column_name in ('display_text', 'output_preview')
            then 'Bounded human/agent-oriented preview; use instead of full payloads by default.'
        when columns.column_name = 'arguments_json'
            then 'Complete serialized tool arguments; potentially large and sensitive.'
        when columns.column_name in ('working_directory', 'repository_root')
            then 'Local path supplied by the source harness.'
        when columns.column_name = 'project_name'
            then 'Native project name when available; otherwise the most specific path basename.'
        when columns.column_name = 'project_root'
            then 'Primary matched project root for this session.'
        when columns.column_name = 'project_evidence'
            then 'Why this session was assigned to the project: explicit, native path, root match, or CWD.'
        when columns.column_name like '%timestamp_utc' or columns.column_name like '%_at_utc'
            then 'Absolute UTC timestamp stored as TIMESTAMPTZ.'
        when columns.column_name like '%_seq' or columns.column_name like '%ordinal%'
            then 'Provider-order position within the containing session or stream.'
        else 'Normalized column exposed by the history query surface.'
    end as description,
    case
        when columns.column_name = 'harness_key' then (
            select string_agg(value, ', ' order by value)
            from (select distinct harness_key as value from history_sessions where harness_key is not null)
        )
        when columns.column_name = 'session_kind' then (
            select string_agg(value, ', ' order by value)
            from (select distinct session_kind as value from history_sessions where session_kind is not null)
        )
        when columns.column_name = 'role' then (
            select string_agg(value, ', ' order by value)
            from (select distinct role as value from history_messages where role is not null)
        )
        when columns.column_name = 'authorship' then (
            select string_agg(value, ', ' order by value)
            from (select distinct authorship as value from history_messages where authorship is not null)
        )
        when columns.column_name = 'parent_relation' then (
            select string_agg(value, ', ' order by value)
            from (
                select distinct parent_relation as value
                from history_sessions
                where parent_relation is not null
            )
        )
        when columns.column_name = 'project_kind' then (
            select string_agg(value, ', ' order by value)
            from (select distinct project_kind as value from history_projects where project_kind is not null)
        )
        when columns.column_name = 'project_evidence' then (
            select string_agg(value, ', ' order by value)
            from (
                select distinct project_evidence as value
                from history_sessions
                where project_evidence is not null
            )
        )
        else null
    end as common_values,
    columns.column_name in ('text', 'display_text', 'arguments_json', 'output_preview', 'summary_text')
        as is_large_text,
    case
        when columns.column_name in ('text', 'display_text', 'arguments_json',
                                     'output_preview', 'summary_text') then 'content'
        when columns.column_name in ('working_directory', 'repository_root', 'project_root', 'root_paths')
            then 'local_path'
        when columns.column_name like '%native%id' then 'provider_identifier'
        else 'ordinary'
    end as data_class
from information_schema.columns columns
where columns.table_schema = 'main'
  and columns.table_name in (
      'history_sessions',
      'history_messages',
      'history_timeline',
      'history_event_relations',
      'history_tool_activity',
      'history_compactions',
      'history_context_epochs',
      'history_session_relations',
      'history_projects',
      'history_turn_pairs',
      'event_details', 'turn_contexts', 'token_events', 'usage_events'
  )
order by columns.table_name, columns.ordinal_position;

create or replace view history_help as
select *
from (values
    ('event_details', 'view',
     'Provider timestamps, timezone offsets, resolution basis and native turn IDs.',
     'select event_id, profile, timestamp_utc, timestamp_offset_minutes, timestamp_basis from event_details order by timestamp_utc desc limit 20'),
    ('turn_contexts', 'view',
     'Provider turn settings including model, effort, working directory and collaboration mode.',
     'select event_id, profile, model, effort, cwd, collaboration_mode_json from turn_contexts order by timestamp_utc desc limit 20'),
    ('token_events', 'view',
     'Cumulative and last-turn token counts with the associated provider context.',
     'select event_id, profile, context_line_no, total_input_tokens, last_input_tokens, model_context_window from token_events order by timestamp_utc desc limit 20'),
    ('usage_events', 'view',
     'Deduplicated per-event usage for daily, weekly and model analysis.',
     'select profile_id, model, input_tokens, cached_input_tokens, output_tokens from usage_events limit 20'),
    ('history_turn_pairs', 'view',
     'One conversation user turn and its first final assistant answer before the next user turn; ' ||
     'no text is copied.',
     'select pairs.turn_ordinal, pairs.question_message_id, pairs.answer_message_id, ' ||
     'pairs.question_authorship, pairs.is_answered ' ||
     'from history_turn_pairs pairs where pairs.session_id = 123 ' ||
     'order by pairs.turn_ordinal limit 50'),
    ('history_projects', 'view',
     'Discovered native projects and exact-CWD fallbacks with roots and active session counts.',
     'select project_id, harness_key, project_name, project_kind, root_paths, session_count ' ||
     'from history_projects where active order by session_count desc limit 50'),
    ('history_sessions', 'view',
     'One row per active session with conversation, parent, location, and count fields.',
     'select session_id, harness_key, project_name, project_root, session_kind, native_session_kind, ' ||
     'session_kind_evidence, agent_label, model, last_event_at_utc, ' ||
     'message_count from history_sessions order by last_event_at_utc desc limit 20'),
    ('history_messages', 'view',
     'Normalized messages with session/agent attribution, role, authorship, epoch, and text.',
     'select message_id, session_id, agent_label, parent_relation, timestamp_utc, role, authorship, ' ||
     'left(text, 400) as text_preview from history_messages ' ||
     'where is_searchable and role = ''user'' order by timestamp_utc desc limit 20'),
    ('history_timeline', 'view',
     'All events in provider order with a compact display_text field.',
     'select event_id, session_seq, timestamp_utc, event_kind, role, tool_name, ' ||
     'left(display_text, 400) as display_preview from history_timeline ' ||
     'where session_id = 123 order by session_seq limit 50'),
    ('history_event_relations', 'view',
     'Provider-supplied child-to-parent event edges for branched histories.',
     'select child_event_id, parent_event_id, relation_kind, evidence_kind ' ||
     'from history_event_relations where session_id = 123 limit 50'),
    ('history_tool_activity', 'view',
     'Tool calls paired with their closest correlated result.',
     'select session_id, called_at_utc, tool_name, ' ||
     'left(arguments_json, 400) as arguments_preview, output_preview, has_result ' ||
     'from history_tool_activity where tool_name = ''Read'' ' ||
     'order by called_at_utc desc limit 20'),
    ('history_compactions', 'view',
     'Compaction boundaries and available summaries.',
     'select event_id, session_seq, timestamp_utc, context_epoch_ordinal, ' ||
     'left(summary_text, 400) as summary_preview from history_compactions ' ||
     'where session_id = 123 order by session_seq limit 20'),
    ('history_context_epochs', 'view',
     'Initial and post-compaction context regions for each session.',
     'select context_epoch_id, context_epoch_ordinal, trigger_kind, ' ||
     'starts_at_session_seq, ends_at_session_seq from history_context_epochs ' ||
     'where session_id = 123 order by context_epoch_ordinal limit 20'),
    ('history_session_relations', 'view',
     'Resolved and unresolved parent, spawn, fork, and continuation assertions.',
     'select relation_kind, child_session_id, parent_session_id, is_resolved, evidence_kind ' ||
     'from history_session_relations where child_session_id = 123 limit 20'),
    ('history_dictionary', 'view',
     'Column types, meanings, observed common values, and data-sensitivity hints for history views.',
     'select surface, column_name, data_type, description, common_values, data_class ' ||
     'from history_dictionary where surface = ''history_messages'' ' ||
     'order by ordinal_position limit 50'),
    ('history_messages_for_session', 'table_macro',
     'Ordered messages for one BIGINT session ID.',
     'select message_id, timestamp_utc, role, authorship, ' ||
     'left(text, 400) as text_preview from history_messages_for_session(123) limit 50'),
    ('history_messages_for_conversation', 'table_macro',
     'Ordered, agent-attributed messages for one sparse conversation-group ID.',
     'select message_id, session_id, agent_label, parent_relation, timestamp_utc, role, authorship, ' ||
     'left(text, 400) as text_preview ' ||
     'from history_messages_for_conversation(123) limit 50'),
    ('history_descendant_sessions', 'table_macro',
     'A root session plus every recursively related descendant.',
     'select session_id, native_session_id, session_kind, lineage_depth, message_count ' ||
     'from history_descendant_sessions(123) limit 50'),
    ('history_message_context', 'table_macro',
     'A target message plus adjacent messages, target marker, and relative message offset.',
     'select message_id, is_target, relative_message_offset, timestamp_utc, role, authorship, ' ||
     'left(text, 400) as text_preview from history_message_context(123, 3, 3) limit 20'),
    ('history_event_context', 'table_macro',
     'A target event plus adjacent timeline events, including intervening tool activity.',
     'select event_id, is_target, relative_event_offset, event_kind, role, tool_name, ' ||
     'left(display_text, 400) as display_preview from history_event_context(123, 10, 10) limit 50'),
    ('history_event_branch', 'table_macro',
     'Root-to-leaf event branch reconstructed from adapter-evidenced parent edges.',
     'select event_id, branch_ordinal, distance_from_leaf, event_kind, role, tool_name, ' ||
     'left(display_text, 400) as display_preview from history_event_branch(123) limit 100')
) help(surface, object_kind, description, example_sql);

