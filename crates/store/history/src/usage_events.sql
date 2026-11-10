WITH candidates AS (
 SELECT te.*,s.profile_id,s.subagent AS is_subagent,cs.harness_key AS source_kind,
  CASE WHEN nullif(te.turn_id,'') IS NOT NULL THEN te.turn_id||':'||cast(te.token_ordinal AS VARCHAR) ELSE te.path||':'||cast(te.line_no AS VARCHAR) END AS usage_key,
  row_number() OVER (PARTITION BY s.profile_id, CASE WHEN nullif(te.turn_id,'') IS NOT NULL THEN te.turn_id||':'||cast(te.token_ordinal AS VARCHAR) ELSE te.path||':'||cast(te.line_no AS VARCHAR) END ORDER BY s.subagent,te.timestamp_utc,te.path,te.line_no) AS duplicate_rank
 FROM token_events te JOIN sessions s ON s.id=te.canonical_session_id JOIN canonical_sessions cs ON cs.id=s.id
 WHERE (coalesce(te.last_input_tokens,0)>0 OR coalesce(te.last_output_tokens,0)>0)
   AND NOT(s.subagent AND nullif(te.turn_id,'') IS NULL AND te.context_line_no IS NULL)
   /* usage scope */
)
SELECT event_id AS id,profile_id,path,line_no,thread_id,session_id,ts,timestamp_utc,turn_id,token_ordinal,
 coalesce(nullif(model,''),'unknown') AS model,coalesce(nullif(effort,''),'unknown') AS effort,is_subagent,source_kind,
 coalesce(last_input_tokens,0) AS input_tokens,
 least(coalesce(last_cached_input_tokens,0),coalesce(last_input_tokens,0)) AS cached_input_tokens,
 least(coalesce(last_cache_write_input_tokens,0),greatest(input_tokens-cached_input_tokens,0)) AS cache_write_input_tokens,
 greatest(input_tokens-cached_input_tokens-cache_write_input_tokens,0) AS uncached_input_tokens,
 coalesce(last_output_tokens,0) AS output_tokens,
 least(coalesce(last_reasoning_output_tokens,0),output_tokens) AS thinking_tokens,
 input_tokens+output_tokens AS total_tokens,usage_key
FROM candidates WHERE duplicate_rank=1
