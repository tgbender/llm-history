-- Seed once for the entire append group. A source owns one sequence range
-- within each session. The latest epoch's committed end points to a projected
-- event even when raw-only records follow it in the archive.
INSERT INTO temp.projection_epoch_tail
WITH append_epoch_ends AS MATERIALIZED (
    SELECT s.session_id,s.source_id,f.generation_id,f.start_ordinal,
           max(ep.ordinal) epoch_ordinal,arg_max(ep.ends_at_event_id,ep.ordinal) end_id
    FROM temp.capture_finish f
    JOIN canonical_session_sources s
      ON s.source_id=f.source_id AND s.current_generation_id=f.generation_id AND s.active
    JOIN canonical_context_epochs ep
      ON ep.session_id=s.session_id AND ep.active AND ep.ends_at_event_id IS NOT NULL
      AND ep.ordinal>=s.sequence_base AND ep.ordinal-s.sequence_base<4294967296
    WHERE f.kind='append' AND f.end_ordinal>f.start_ordinal
    GROUP BY s.session_id,s.source_id,f.generation_id,f.start_ordinal
), end_events AS MATERIALIZED (
    -- A membership probe lets DuckDB use the existing event ID index.
    SELECT id,event_kind,source_id,session_id,generation_id,ordinal,fact_index,active
    FROM events WHERE id IN (SELECT end_id FROM append_epoch_ends)
)
SELECT a.session_id,a.source_id,a.epoch_ordinal+(e.event_kind='compaction')::BIGINT
FROM append_epoch_ends a
JOIN end_events e ON e.id=a.end_id AND e.active AND e.fact_index=0
  AND e.source_id=a.source_id AND e.session_id=a.session_id
  AND e.generation_id=a.generation_id AND e.ordinal<=a.start_ordinal;
