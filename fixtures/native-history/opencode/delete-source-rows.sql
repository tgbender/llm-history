-- Native deletion cascades to transcript rows and blob references; blob GC is separate.
DELETE FROM session_v2 WHERE id = 'ses_fixture';
