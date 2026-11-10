-- Legacy transcript tables from the pinned OpenCode V1 bootstrap migration.
PRAGMA foreign_keys = ON;
CREATE TABLE `project` (
          `id` text PRIMARY KEY,
          `worktree` text NOT NULL,
          `vcs` text,
          `name` text,
          `icon_url` text,
          `icon_color` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `time_initialized` integer,
          `sandboxes` text NOT NULL
        );
CREATE TABLE `session` (
          `id` text PRIMARY KEY,
          `project_id` text NOT NULL,
          `parent_id` text,
          `slug` text NOT NULL,
          `directory` text NOT NULL,
          `title` text NOT NULL,
          `version` text NOT NULL,
          `share_url` text,
          `summary_additions` integer,
          `summary_deletions` integer,
          `summary_files` integer,
          `summary_diffs` text,
          `revert` text,
          `permission` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `time_compacting` integer,
          `time_archived` integer,
          CONSTRAINT `fk_session_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `message` (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `fk_message_session_id_session_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `part` (
          `id` text PRIMARY KEY,
          `message_id` text NOT NULL,
          `session_id` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `fk_part_message_id_message_id_fk` FOREIGN KEY (`message_id`) REFERENCES `message`(`id`) ON DELETE CASCADE
        );
INSERT INTO project (id, worktree, time_created, time_updated, sandboxes) VALUES ('fixture-project', '/fixture/project', 1794078000000, 1794078000000, '[]');
INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated) VALUES ('ses_fixture_v1', 'fixture-project', 'fixture-v1', '/fixture/project', 'Fixture V1', '1.0.0', 1794078000000, 1794078000002);
INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES ('msg_fixture_v1_user', 'ses_fixture_v1', 1794078000000, 1794078000000, '{"role":"user","time":{"created":1794078000000},"agent":"build","model":{"providerID":"fixture","modelID":"fixture-model"}}');
INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES ('msg_fixture_v1_assistant', 'ses_fixture_v1', 1794078000001, 1794078000002, '{"role":"assistant","time":{"created":1794078000001,"completed":1794078000002},"agent":"build","mode":"build","parentID":"msg_fixture_v1_user","modelID":"fixture-model","providerID":"fixture","path":{"cwd":"/fixture/project","root":"/fixture/project"},"cost":0,"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"finish":"stop"}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_01', 'msg_fixture_v1_user', 'ses_fixture_v1', 1794078000000, 1794078000000, '{"type":"text","text":"Fixture greeting: café 🦀\n"}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_02', 'msg_fixture_v1_assistant', 'ses_fixture_v1', 1794078000001, 1794078000002, '{"type":"text","text":"Fixture greeting: café 🦀\n"}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_03', 'msg_fixture_v1_user', 'ses_fixture_v1', 1794078000003, 1794078000003, '{"type":"file","mime":"image/png","filename":"pixel.png","url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg=="}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_04', 'msg_fixture_v1_user', 'ses_fixture_v1', 1794078000004, 1794078000004, '{"type":"agent","name":"fixture","source":{"value":"@fixture","start":0,"end":8}}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_05', 'msg_fixture_v1_user', 'ses_fixture_v1', 1794078000005, 1794078000005, '{"type":"subtask","prompt":"Read the sample.","description":"Fixture subtask","agent":"fixture"}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_06', 'msg_fixture_v1_user', 'ses_fixture_v1', 1794078000006, 1794078000006, '{"type":"compaction","auto":true,"overflow":false,"tail_start_id":"msg_fixture_v1_user"}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_07', 'msg_fixture_v1_assistant', 'ses_fixture_v1', 1794078000007, 1794078000007, '{"type":"step-start","snapshot":"fixture-before"}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_08', 'msg_fixture_v1_assistant', 'ses_fixture_v1', 1794078000008, 1794078000008, '{"type":"reasoning","text":"Inspect the sample.","time":{"start":1794078000000,"end":1794078000001}}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_09', 'msg_fixture_v1_assistant', 'ses_fixture_v1', 1794078000009, 1794078000009, '{"type":"snapshot","snapshot":"fixture-before"}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_10', 'msg_fixture_v1_assistant', 'ses_fixture_v1', 1794078000010, 1794078000010, '{"type":"patch","hash":"fixture-after","files":["/fixture/project/hello.txt"]}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_11', 'msg_fixture_v1_assistant', 'ses_fixture_v1', 1794078000011, 1794078000011, '{"type":"retry","attempt":1,"time":{"created":1794078000000},"error":{"name":"APIError","data":{"message":"Synthetic retry","statusCode":429,"isRetryable":true}}}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_12', 'msg_fixture_v1_assistant', 'ses_fixture_v1', 1794078000012, 1794078000012, '{"type":"step-finish","reason":"stop","cost":0,"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}}}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_13', 'msg_fixture_v1_assistant', 'ses_fixture_v1', 1794078000013, 1794078000013, '{"type":"tool","callID":"fixture_pending","tool":"read","state":{"status":"pending","input":{"path":"hello.txt"},"raw":"{}"}}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_14', 'msg_fixture_v1_assistant', 'ses_fixture_v1', 1794078000014, 1794078000014, '{"type":"tool","callID":"fixture_running","tool":"read","state":{"status":"running","input":{"path":"hello.txt"},"time":{"start":1794078000000}}}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_15', 'msg_fixture_v1_assistant', 'ses_fixture_v1', 1794078000015, 1794078000015, '{"type":"tool","callID":"fixture_completed","tool":"read","state":{"status":"completed","input":{"path":"hello.txt"},"time":{"start":1794078000000,"end":1794078000001},"output":"Fixture greeting: café 🦀\n","title":"Fixture read","metadata":{}}}');
INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('prt_fixture_16', 'msg_fixture_v1_assistant', 'ses_fixture_v1', 1794078000016, 1794078000016, '{"type":"tool","callID":"fixture_error","tool":"read","state":{"status":"error","input":{"path":"hello.txt"},"time":{"start":1794078000000,"end":1794078000001},"error":"Synthetic error"}}');
