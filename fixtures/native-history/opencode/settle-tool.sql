-- Mutate existing message/part rows; no higher message seq is allocated.
UPDATE session_part SET data = '{"type":"tool","id":"call_fixture","name":"read","state":{"status":"completed","input":{"filePath":"hello.txt"},"content":[{"type":"text","text":"Fixture greeting: café 🦀\n"},{"type":"file","uri":"media:2640059609118b695c139804676372eca75c30d30e5906775902d13e44f5356c","mime":"image/png","name":"pixel.png"}]},"time":{"created":1794078000000,"ran":1794078000001,"completed":1794078000030}}', time_updated = 1794078000030 WHERE message_id = 'msg_split' AND ordinal = 1;
DELETE FROM session_part WHERE message_id = 'msg_split' AND ordinal = 2;
UPDATE session_message SET data = json_set(data, '$.time.completed', 1794078000030), time_updated = 1794078000030 WHERE id = 'msg_split';
INSERT INTO blob_ref (session_id, message_id, hash, seq) VALUES ('ses_fixture', 'msg_split', '2640059609118b695c139804676372eca75c30d30e5906775902d13e44f5356c', 11);
UPDATE session_v2 SET title = 'Fixture renamed', time_updated = 1794078000030 WHERE id = 'ses_fixture';
