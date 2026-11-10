-- A catalog-only commit in the live WAL; the selected path and rollout stay put.
UPDATE threads SET title = 'Fixture metadata-only rename', updated_at = updated_at + 1
WHERE id = '00000000-0000-4000-8000-000000000002';
