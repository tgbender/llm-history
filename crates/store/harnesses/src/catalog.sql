CREATE TABLE IF NOT EXISTS native_project_assignments (profile_id VARCHAR,session_native VARCHAR,project_native VARCHAR,PRIMARY KEY(profile_id,session_native,project_native));
CREATE TABLE IF NOT EXISTS catalog_snapshots (id BIGINT PRIMARY KEY,profile_id VARCHAR,uri VARCHAR,format VARCHAR,digest VARCHAR,data VARCHAR,captured_at TIMESTAMPTZ,current BOOLEAN);
