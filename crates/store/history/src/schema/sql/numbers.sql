CREATE SEQUENCE message_no_seq START 1;
CREATE SEQUENCE session_no_seq START 1;

CREATE TABLE message_numbers (
    message_no BIGINT PRIMARY KEY DEFAULT nextval('message_no_seq'),
    event_id BIGINT UNIQUE NOT NULL
);

CREATE TABLE session_numbers (
    session_no BIGINT PRIMARY KEY DEFAULT nextval('session_no_seq'),
    session_id BIGINT UNIQUE NOT NULL
);
