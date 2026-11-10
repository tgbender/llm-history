-- Identical repairs and snapshots keep the archived generation and raw observation IDs.
CREATE OR REPLACE TEMP TABLE identical_repairs AS
WITH repairs AS (
    SELECT source_id,generation_id,old_generation_id,complete
    FROM temp.capture_finish
    WHERE kind IN ('repair','snapshot') AND old_generation_id IS NOT NULL
), current_rows AS (
    SELECT f.source_id,r.ordinal,r.start_byte,r.end_byte,r.payload_hash
    FROM repairs f JOIN native_observations r ON r.generation_id=f.generation_id
), prior_rows AS (
    SELECT f.source_id,r.ordinal,r.start_byte,r.end_byte,r.payload_hash
    FROM repairs f JOIN native_observations r ON r.generation_id=f.old_generation_id
), differences AS (
    (SELECT * FROM current_rows EXCEPT ALL SELECT * FROM prior_rows)
    UNION ALL
    (SELECT * FROM prior_rows EXCEPT ALL SELECT * FROM current_rows)
)
SELECT f.* FROM repairs f ANTI JOIN differences d USING (source_id);

UPDATE events e SET generation_id=i.old_generation_id,raw_record_id=r.raw_id
FROM temp.identical_repairs i
JOIN native_observations r ON r.generation_id=i.old_generation_id
WHERE e.source_id=i.source_id AND e.generation_id=i.generation_id AND r.ordinal=e.ordinal;

UPDATE canonical_session_sources s SET current_generation_id=i.old_generation_id
FROM temp.identical_repairs i WHERE s.source_id=i.source_id AND s.current_generation_id=i.generation_id;

DELETE FROM native_observations r USING temp.identical_repairs i WHERE r.generation_id=i.generation_id;
DELETE FROM generations g USING temp.identical_repairs i WHERE g.id=i.generation_id;

UPDATE generations g SET bytes=f.end_byte,lines=f.end_ordinal,complete=f.complete
FROM temp.capture_finish f LEFT JOIN temp.identical_repairs i USING (source_id)
WHERE g.id=coalesce(i.old_generation_id,f.generation_id);

INSERT INTO captures (id,source_id,generation_id,location_uri,encoding,physical_cutoff,
                      start_ordinal,start_decoded_byte,end_ordinal,end_decoded_byte,complete)
SELECT capture_id,source_id,generation_id,uri,encoding,physical_cutoff,
       start_ordinal,start_byte,end_ordinal,end_byte,complete
FROM temp.capture_finish f
ANTI JOIN temp.identical_repairs i USING (source_id)
WHERE f.kind<>'append' OR f.end_ordinal>f.start_ordinal;

INSERT INTO sources (id,profile_id,source_key,uri,committed,ordinal,prefix_hash,parser_state,
                     generation,missing,observed_size,observed_mtime_ns,head_xxh64,boundary_xxh64,parser_version)
SELECT f.source_id,f.profile_id,f.source_key,f.uri,f.end_byte,f.end_ordinal,f.source_token,f.parser_state,
       f.generation-(i.source_id IS NOT NULL)::BIGINT,false,f.physical_cutoff,f.mtime_ns,
       f.head_xxh64,f.boundary_xxh64,f.parser_version
FROM temp.capture_finish f LEFT JOIN temp.identical_repairs i USING (source_id)
ON CONFLICT (id) DO UPDATE SET
    source_key=excluded.source_key,uri=excluded.uri,committed=excluded.committed,
    ordinal=excluded.ordinal,prefix_hash=excluded.prefix_hash,parser_state=excluded.parser_state,
    generation=excluded.generation,missing=false,observed_size=excluded.observed_size,
    observed_mtime_ns=excluded.observed_mtime_ns,head_xxh64=excluded.head_xxh64,
    boundary_xxh64=excluded.boundary_xxh64,parser_version=excluded.parser_version;

DROP TABLE temp.identical_repairs;
DROP TABLE temp.capture_finish;
