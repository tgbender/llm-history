-- Synthetic Cursor CLI store.db, schema version 1.
PRAGMA journal_mode = WAL;
PRAGMA user_version = 1;
CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
BEGIN;
-- shared JSON user.
INSERT INTO blobs VALUES ('1d03332a39b6af7e90aaf9bac9a430745a703660281e174791521b836a080421',
    CAST('{"role":"user","content":[{"type":"text","text":"Run two shell checks for the fixture."}]}' AS BLOB));
-- shared native user.
INSERT INTO blobs VALUES ('ce996f397c0b4a52c431e08bf425a883cd80cdf690227d3868b5c670fd1120ae',
    X'0a2552756e2074776f207368656c6c20636865636b7320666f722074686520666978747572652e122430303030303030302d303030302d343030302d383030302d3030303030303030303033322001c80180a8b2a1f433');
-- superseded JSON call.
INSERT INTO blobs VALUES ('047db236b8470f7e4922d726dadee999f9bebd962386ed7412c04d1da61e0af3',
    CAST('{"role":"assistant","content":[{"type":"tool-call","toolName":"Shell","toolCallId":"fixture-call-amber","args":{"command":"printf amber"}}]}' AS BLOB));
-- superseded JSON result, same call ID as the active root.
INSERT INTO blobs VALUES ('69d1c8dc549c36acc6f7452bb334774e4662260a58e305d19a277f7109c87c96',
    CAST('{"role":"tool","content":[{"type":"tool-result","toolName":"Shell","toolCallId":"fixture-call-amber","result":"Abandoned result: stale amber.","isError":false}]}' AS BLOB));
-- superseded JSON answer.
INSERT INTO blobs VALUES ('74eb6694fe0c1a85f23d91ee6de685cf235ec2a15063095ba089270afaf0272c',
    CAST('{"role":"assistant","content":[{"type":"text","text":"Abandoned reply: both checks succeeded."}]}' AS BLOB));
-- superseded native shell step.
INSERT INTO blobs VALUES ('504c5e354b8665bea04cab3240a81e5880b53b052525caab0b1cf3e11742caf1',
    X'1291010a7a0a330a0c7072696e746620616d626572120f2f666978747572652f637572736f722212666978747572652d63616c6c2d616d62657212430a410a0c7072696e746620616d626572120f2f666978747572652f637572736f7218002a1e4162616e646f6e656420726573756c743a207374616c6520616d6265722eca0312666978747572652d63616c6c2d616d626572');
-- superseded native answer.
INSERT INTO blobs VALUES ('6623a9b11bbd6c3dc1c5280254fbbc1e18442df5c054fa939bd770f4d313fedb',
    X'0a300a274162616e646f6e6564207265706c793a20626f746820636865636b73207375636365656465642e10e8afb2a1f433');
-- superseded native turn.
INSERT INTO blobs VALUES ('ee696ced3ccf8b829b32512b6d978f8148b41cb93ea7bc21180d1c02547188ec',
    X'0a660a20ce996f397c0b4a52c431e08bf425a883cd80cdf690227d3868b5c670fd1120ae1220504c5e354b8665bea04cab3240a81e5880b53b052525caab0b1cf3e11742caf112206623a9b11bbd6c3dc1c5280254fbbc1e18442df5c054fa939bd770f4d313fedb');
-- superseded root, retained but not selected.
INSERT INTO blobs VALUES ('1d7840870e641b8a16359c48905a202fcb1f31165e0093e2d16087d7695d09c4',
    X'0a201d03332a39b6af7e90aaf9bac9a430745a703660281e174791521b836a0804210a20047db236b8470f7e4922d726dadee999f9bebd962386ed7412c04d1da61e0af30a2069d1c8dc549c36acc6f7452bb334774e4662260a58e305d19a277f7109c87c960a2074eb6694fe0c1a85f23d91ee6de685cf235ec2a15063095ba089270afaf0272c4220ee696ced3ccf8b829b32512b6d978f8148b41cb93ea7bc21180d1c02547188ec4a1666696c653a2f2f2f666978747572652f637572736f725001b20103636c69d00180a8b2a1f433');
-- active JSON answer, inserted before its calls on purpose.
INSERT INTO blobs VALUES ('91c394371f36b5ad3443cf9be093824d6eed0373b4466df7b75bb77be37e3fcf',
    CAST('{"role":"assistant","content":[{"type":"text","text":"Amber succeeded; the input file was missing."}]}' AS BLOB));
-- active JSON calls.
INSERT INTO blobs VALUES ('dcffa01dd9f75b8df1ac81dc3f5aa8269c6398e980f497ad7fb5b1f03931470c',
    CAST('{"role":"assistant","content":[{"type":"tool-call","toolName":"Shell","toolCallId":"fixture-call-amber","args":{"command":"printf amber"}},{"type":"tool-call","toolName":"Shell","toolCallId":"fixture-call-missing","args":{"command":"cat missing.txt"}}]}' AS BLOB));
-- active JSON success result.
INSERT INTO blobs VALUES ('a15973da5a859aa4999893564732608f3ea92f935ed25f6cd48bc4d388072f6e',
    CAST('{"role":"tool","content":[{"type":"tool-result","toolName":"Shell","toolCallId":"fixture-call-amber","result":"amber","isError":false}]}' AS BLOB));
-- active JSON failure result.
INSERT INTO blobs VALUES ('fed0a41e32b83178ae629f074cac1d4e3ad849041dc9ca59f3cef4a27399dfd4',
    CAST('{"role":"tool","content":[{"type":"tool-result","toolName":"Shell","toolCallId":"fixture-call-missing","result":"missing.txt: No such file or directory","isError":true}]}' AS BLOB));
-- active native shell step, success.
INSERT INTO blobs VALUES ('c5eafddebdedcd91dc3a5b6c3f8648948fc2b897a8c9a2adbf03a22f172828df',
    X'12780a610a330a0c7072696e746620616d626572120f2f666978747572652f637572736f722212666978747572652d63616c6c2d616d626572122a0a280a0c7072696e746620616d626572120f2f666978747572652f637572736f7218002a05616d626572ca0312666978747572652d63616c6c2d616d626572');
-- active native shell step, failure.
INSERT INTO blobs VALUES ('4851ce2bd9a6d4b0cb0c1d7b7d1ccf33e569bdc4ab3a2286a39ddf0dcc38b4af',
    X'12a4010a8a010a380a0f636174206d697373696e672e747874120f2f666978747572652f637572736f722214666978747572652d63616c6c2d6d697373696e67124e124c0a0f636174206d697373696e672e747874120f2f666978747572652f637572736f72180132266d697373696e672e7478743a204e6f20737563682066696c65206f72206469726563746f7279ca0314666978747572652d63616c6c2d6d697373696e67');
-- active native answer.
INSERT INTO blobs VALUES ('fbe7362879d9faf60e65c2e7a6d93140893cb9146a2bc9f1b17783897b3ebc18',
    X'0a350a2c416d626572207375636365656465643b2074686520696e7075742066696c6520776173206d697373696e672e10d0b7b2a1f433');
-- active native turn.
INSERT INTO blobs VALUES ('dd6795dc2e95d4941e9bd1dde7de612a1129fbd212b5334c827002521ff64660',
    X'0a88010a20ce996f397c0b4a52c431e08bf425a883cd80cdf690227d3868b5c670fd1120ae1220c5eafddebdedcd91dc3a5b6c3f8648948fc2b897a8c9a2adbf03a22f172828df12204851ce2bd9a6d4b0cb0c1d7b7d1ccf33e569bdc4ab3a2286a39ddf0dcc38b4af1220fbe7362879d9faf60e65c2e7a6d93140893cb9146a2bc9f1b17783897b3ebc18');
-- active root; failed call completed first in the message stream.
INSERT INTO blobs VALUES ('3fac6f5dbadb31d9113ee92624d8dda788143381218e0bd46d2cc17e06e53c5c',
    X'0a201d03332a39b6af7e90aaf9bac9a430745a703660281e174791521b836a0804210a20dcffa01dd9f75b8df1ac81dc3f5aa8269c6398e980f497ad7fb5b1f03931470c0a20fed0a41e32b83178ae629f074cac1d4e3ad849041dc9ca59f3cef4a27399dfd40a20a15973da5a859aa4999893564732608f3ea92f935ed25f6cd48bc4d388072f6e0a2091c394371f36b5ad3443cf9be093824d6eed0373b4466df7b75bb77be37e3fcf4220dd6795dc2e95d4941e9bd1dde7de612a1129fbd212b5334c827002521ff646604a1666696c653a2f2f2f666978747572652f637572736f725001b20103636c69d00180a8b2a1f433');
-- Cursor stores this JSON as hex text, not as a JSON SQLite value.
INSERT INTO meta VALUES ('0', lower(hex('{"agentId":"00000000-0000-4000-8000-000000000031","latestRootBlobId":"3fac6f5dbadb31d9113ee92624d8dda788143381218e0bd46d2cc17e06e53c5c","name":"Synthetic Cursor shell checks","createdAt":1783555200000,"workspacePath":"/fixture/cursor","mode":"default"}')));
COMMIT;
