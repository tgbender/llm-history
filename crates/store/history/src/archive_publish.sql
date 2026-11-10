-- Select new document keys once for the whole staged group. Keep compressed
-- bytes in staging and retain the representation of any existing document.
CREATE TEMP TABLE cas_new_documents AS
SELECT s.hash FROM temp.stage_native_documents s
ANTI JOIN native_documents d USING (hash);

-- Only bodies referenced by a new document are eligible for insertion.
INSERT INTO content_objects
SELECT s.* FROM temp.stage_content_objects s
SEMI JOIN (
    SELECT p.content_id FROM temp.stage_content_parts p
    SEMI JOIN temp.cas_new_documents d ON d.hash=p.document_hash
) referenced ON referenced.content_id=s.id
ANTI JOIN content_objects o USING (id);

INSERT INTO content_parts
SELECT p.* FROM temp.stage_content_parts p
SEMI JOIN temp.cas_new_documents d ON d.hash=p.document_hash;

INSERT INTO native_documents
SELECT s.* FROM temp.stage_native_documents s
SEMI JOIN temp.cas_new_documents d USING (hash);

DROP TABLE temp.cas_new_documents;
DELETE FROM temp.stage_native_documents;
DELETE FROM temp.stage_content_parts;
DELETE FROM temp.stage_content_objects;
