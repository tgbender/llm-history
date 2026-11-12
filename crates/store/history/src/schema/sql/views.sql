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
    child_numbers.session_no as child_session_id,
    children.id as child_session_key,
    children.native_session_id as child_native_session_id,
    children.harness_key,
    parent_numbers.session_no as parent_session_id,
    relations.target_session_id as parent_session_key,
    coalesce(parents.native_session_id, relations.target_native_session_id)
        as parent_native_session_id,
    relations.target_session_id is not null as is_resolved
from canonical_session_relations relations
join canonical_sessions children on children.id = relations.source_session_id
left join canonical_sessions parents on parents.id = relations.target_session_id
left join session_numbers child_numbers on child_numbers.session_id = children.id
left join session_numbers parent_numbers on parent_numbers.session_id = relations.target_session_id;

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
                partition by relations.child_session_key
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
    select parent_session_key, count(*) as child_session_count
    from history_session_relations
    where active and is_resolved
    group by parent_session_key
)
select
    numbers.session_no as session_id,
    sessions.id as session_key,
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
    coalesce(child_counts.child_session_count, 0) as child_session_count
from canonical_sessions sessions
left join session_numbers numbers on numbers.session_id = sessions.id
left join canonical_conversation_memberships memberships
  on memberships.session_id = sessions.id and memberships.is_primary
left join primary_relations on primary_relations.child_session_key = sessions.id
left join primary_projects on primary_projects.session_id = sessions.id
left join message_counts on message_counts.session_id = sessions.id
left join tool_counts on tool_counts.session_id = sessions.id
left join compaction_counts on compaction_counts.session_id = sessions.id
left join child_counts on child_counts.parent_session_key = sessions.id
where sessions.active;

create or replace view history_messages as
select
    numbers.message_no as message_id,
    messages.event_id,
    sessions.session_id,
    messages.session_id as session_key,
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
    hex(messages.content_blake3) as content_blake3_hex
from canonical_messages messages
join history_sessions sessions on sessions.session_key = messages.session_id
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
    session_number.session_no as session_id,
    questions.session_id as session_key,
    memberships.conversation_group_id,
    sessions.harness_key,
    sessions.native_session_id,
    sessions.session_kind,
    questions.turn_ordinal,
    question_numbers.message_no as question_message_id,
    answer_numbers.message_no as answer_message_id,
    questions.event_id as question_event_id,
    answers.event_id as answer_event_id,
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
left join session_numbers session_number on session_number.session_id = questions.session_id
left join message_numbers question_numbers on question_numbers.event_id = questions.event_id
left join message_numbers answer_numbers on answer_numbers.event_id = answers.event_id
left join canonical_conversation_memberships memberships
  on memberships.session_id = questions.session_id and memberships.is_primary;

create or replace view history_timeline as
select
    timeline.event_id,
    sessions.session_id,
    timeline.session_id as session_key,
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
join history_sessions sessions on sessions.session_key = timeline.session_id
left join canonical_context_epochs epochs on epochs.id = timeline.context_epoch_id
left join canonical_compactions compactions on compactions.event_id = timeline.event_id
left join canonical_conversation_memberships memberships
  on memberships.session_id = timeline.session_id and memberships.is_primary;

create or replace view history_event_relations as
select
    relations.id as event_relation_id,
    children.session_id,
    relations.session_id as session_key,
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
    numbers.session_no as session_id,
    paired.session_id as session_key,
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
left join session_numbers numbers on numbers.session_id = paired.session_id
left join canonical_conversation_memberships memberships
  on memberships.session_id = paired.session_id and memberships.is_primary
where paired.result_rank = 1;

create or replace view history_compactions as
select
    compactions.event_id,
    numbers.session_no as session_id,
    compactions.session_id as session_key,
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
left join session_numbers numbers on numbers.session_id = compactions.session_id
left join canonical_context_epochs epochs on epochs.id = compactions.context_epoch_id
left join canonical_conversation_memberships memberships
  on memberships.session_id = compactions.session_id and memberships.is_primary;

create or replace view history_context_epochs as
select
    epochs.id as context_epoch_id,
    numbers.session_no as session_id,
    epochs.session_id as session_key,
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
left join session_numbers numbers on numbers.session_id = epochs.session_id
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
    join history_sessions sessions on sessions.session_key = ancestry.session_id
    where ancestry.ancestor_session_id = (
        select numbers.session_id from session_numbers numbers
        where numbers.session_no = p_root_session_id
    )
    order by ancestry.depth, sessions.started_at_utc nulls last, sessions.session_id
);

create or replace macro history_message_context(p_message_id, p_before, p_after) as table (
    with target as (
        select session_key, message_ordinal
        from history_messages
        where message_id = p_message_id
    )
    select
        messages.*,
        messages.message_id = p_message_id as is_target,
        messages.message_ordinal - target.message_ordinal as relative_message_offset
    from history_messages messages
    join target using(session_key)
    where messages.message_ordinal between
            greatest(target.message_ordinal - greatest(p_before, 0), 1)
        and target.message_ordinal + greatest(p_after, 0)
    order by messages.message_ordinal
);

create or replace macro history_event_context(p_event_id, p_before, p_after) as table (
    with target_session as (
        select session_key
        from history_timeline
        where event_id = p_event_id
    ), ordered as (
        select
            timeline.*,
            row_number() over (
                partition by timeline.session_key
                order by timeline.session_seq, timeline.event_id
            ) as event_ordinal
        from history_timeline timeline
        join target_session using(session_key)
    ), target as (
        select session_key, event_ordinal
        from ordered
        where event_id = p_event_id
    )
    select
        events.*,
        events.event_id = p_event_id as is_target,
        events.event_ordinal - target.event_ordinal as relative_event_offset
    from ordered events
    join target using(session_key)
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
        when columns.column_name in ('session_id', 'parent_session_id', 'child_session_id')
            then 'Short session number: the ID sessions prints and show --session takes.'
        when columns.column_name in ('message_id', 'question_message_id', 'answer_message_id')
            then 'Short message number: the ID search prints and show takes; an answer may be null.'
        when columns.column_name in ('session_key', 'parent_session_key', 'child_session_key',
                                     'event_id', 'question_event_id', 'answer_event_id',
                                     'relation_id', 'conversation_group_id', 'context_epoch_id')
            then 'Stable BIGINT join key generated by llm-history; long, so prefer the short IDs.'
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
