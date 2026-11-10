-- Simulate a missing message blob; the selected root and other blobs remain.
DELETE FROM blobs WHERE id = 'fed0a41e32b83178ae629f074cac1d4e3ad849041dc9ca59f3cef4a27399dfd4';
