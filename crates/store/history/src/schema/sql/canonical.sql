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
