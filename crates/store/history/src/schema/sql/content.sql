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
