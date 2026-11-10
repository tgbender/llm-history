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
