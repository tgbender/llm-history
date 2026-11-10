-- Projects one staged batch, from any number of sources, into the fact table.
-- A source owns a stable 2^32 sequence range in each session, including
-- overlapping captures. Appending or reprocessing never reallocates it.
-- For new ranges, first_staged_row records the Appender's publication order.
CREATE OR REPLACE TEMP TABLE stage_pairs AS
SELECT session_id,source_id,min(rowid) first_staged_row,max(generation_id) generation_id
FROM temp.stage_events WHERE fact_index=0 GROUP BY ALL;
INSERT INTO canonical_session_sources (session_id,source_id,current_generation_id,first_seen_at,last_seen_at,active,sequence_base)
SELECT p.session_id,p.source_id,p.generation_id,now(),now(),true,
 coalesce(existing.sequence_base,
   coalesce((SELECT max(c.sequence_base) FROM canonical_session_sources c WHERE c.session_id=p.session_id),-4294967296)
   +4294967296*count(*) FILTER (WHERE existing.source_id IS NULL) OVER (PARTITION BY p.session_id ORDER BY p.first_staged_row))
FROM stage_pairs p
LEFT JOIN canonical_session_sources existing ON existing.session_id=p.session_id AND existing.source_id=p.source_id
ON CONFLICT(session_id,source_id) DO UPDATE SET current_generation_id=excluded.current_generation_id,last_seen_at=now(),active=true;

CREATE OR REPLACE TEMP TABLE stage_canonical AS
SELECT e.id,e.session_id,e.source_id,s.sequence_base,s.sequence_base+e.ordinal::BIGINT AS session_seq,
 coalesce(t.next_epoch,s.sequence_base)+coalesce(sum(CASE WHEN e.kind='compaction' THEN 1 ELSE 0 END) OVER (PARTITION BY e.session_id,e.source_id ORDER BY e.ordinal ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING),0)::BIGINT AS epoch_ordinal,
 e.kind='compaction' closes_epoch,e.generation_id,e.raw_id
FROM temp.stage_events e
JOIN canonical_session_sources s ON s.source_id=e.source_id AND s.session_id=e.session_id
LEFT JOIN temp.projection_epoch_tail t ON t.session_id=e.session_id AND t.source_id=e.source_id
WHERE e.fact_index=0;

-- A compaction belongs to the epoch it closes. If it is the last fact in
-- this flush, the cursor already points at the next (possibly empty) epoch.
INSERT INTO temp.projection_epoch_tail
SELECT session_id,source_id,max(epoch_ordinal+closes_epoch::BIGINT)
FROM temp.stage_canonical GROUP BY session_id,source_id
ON CONFLICT (session_id,source_id) DO UPDATE SET next_epoch=excluded.next_epoch;

INSERT INTO canonical_context_epochs
SELECT nextval('entity_ids'),session_id,epoch_ordinal,NULL,NULL,
 CASE WHEN epoch_ordinal=sequence_base THEN 'initial' ELSE 'compaction' END,'adapter_derived',true,now(),now()
FROM (SELECT DISTINCT session_id,epoch_ordinal,sequence_base FROM stage_canonical) t
WHERE NOT EXISTS(SELECT 1 FROM canonical_context_epochs ep WHERE ep.session_id=t.session_id AND ep.ordinal=t.epoch_ordinal);

-- Identity columns (source, ordinal, fact index, payload) never change for an ID.
INSERT INTO events (id,source_id,session_id,ordinal,fact_index,payload_hash,kind,ts,role,phase,text,text_hash,model,tool,call_id,input_tokens,output_tokens,cache_read,active,cache_write,excluded,usage_key,effort,thinking_tokens,usage_eligible,generation_id,raw_record_id,session_seq,context_epoch_id,timestamp_utc,timestamp_offset_minutes,timestamp_timezone,timestamp_basis,turn_id,event_kind,native_event_type,native_payload_type,metadata_json,native_item_id,native_parent_id,native_turn_id,arguments_json,agent_protocol)
SELECT e.id,e.source_id,e.session_id,e.ordinal,e.fact_index,e.payload_hash,e.kind,e.ts,e.role,e.phase,e.text,e.text_hash,e.model,e.tool,e.call_id,e.input_tokens,e.output_tokens,e.cache_read,true,e.cache_write,e.excluded,e.usage_key,e.effort,e.thinking_tokens,e.usage_eligible,
 c.generation_id,c.raw_id,c.session_seq,ep.id,try_cast(e.timestamp_utc AS TIMESTAMPTZ),e.timestamp_offset_minutes,e.timestamp_timezone,e.timestamp_basis,e.turn_id,
 CASE WHEN c.id IS NULL THEN NULL WHEN e.kind='compaction' THEN 'compaction' WHEN e.kind='tool_result' OR e.native_payload_type IN ('tool_result','tool-result') THEN 'tool_result' WHEN e.kind='tool_call' THEN 'tool_call' WHEN e.role IN ('user','assistant','system','developer','tool') THEN 'message' WHEN e.kind LIKE '%context%' THEN 'context' ELSE 'harness_event' END,
 nullif(e.native_event_type,''),nullif(e.native_payload_type,''),e.metadata_json,e.native_item_id,e.native_parent_id,e.native_turn_id,e.arguments_json,coalesce(e.agent_protocol,false)
FROM temp.stage_events e
LEFT JOIN stage_canonical c ON c.id=e.id
LEFT JOIN canonical_context_epochs ep ON ep.session_id=c.session_id AND ep.ordinal=c.epoch_ordinal
ON CONFLICT(id) DO UPDATE SET session_id=excluded.session_id,kind=excluded.kind,ts=excluded.ts,role=excluded.role,phase=excluded.phase,text=excluded.text,text_hash=excluded.text_hash,model=excluded.model,tool=excluded.tool,call_id=excluded.call_id,input_tokens=excluded.input_tokens,output_tokens=excluded.output_tokens,cache_read=excluded.cache_read,active=true,cache_write=excluded.cache_write,excluded=excluded.excluded,usage_key=excluded.usage_key,effort=excluded.effort,thinking_tokens=excluded.thinking_tokens,usage_eligible=excluded.usage_eligible,generation_id=excluded.generation_id,raw_record_id=excluded.raw_record_id,session_seq=excluded.session_seq,context_epoch_id=excluded.context_epoch_id,timestamp_utc=excluded.timestamp_utc,timestamp_offset_minutes=excluded.timestamp_offset_minutes,timestamp_timezone=excluded.timestamp_timezone,timestamp_basis=excluded.timestamp_basis,turn_id=excluded.turn_id,event_kind=excluded.event_kind,native_event_type=excluded.native_event_type,native_payload_type=excluded.native_payload_type,metadata_json=excluded.metadata_json,native_item_id=excluded.native_item_id,native_parent_id=excluded.native_parent_id,native_turn_id=excluded.native_turn_id,arguments_json=excluded.arguments_json,agent_protocol=excluded.agent_protocol;

INSERT INTO message_numbers(event_id)
SELECT DISTINCT e.id FROM temp.stage_events st JOIN events e ON e.id=st.id
ANTI JOIN message_numbers n ON n.event_id=e.id
WHERE e.kind='message' AND e.text<>'' AND e.fact_index=0 AND e.session_seq IS NOT NULL AND e.role<>''
ORDER BY e.id;

-- Incremental FTS uses transition-table triggers, which DuckDB 2.0 does not
-- support on ON CONFLICT DO UPDATE statements.
DELETE FROM lexical_documents WHERE id IN (SELECT id FROM temp.stage_lexical);
INSERT INTO lexical_documents SELECT * FROM temp.stage_lexical;
DELETE FROM temp.stage_lexical;
