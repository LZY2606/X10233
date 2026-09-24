PRAGMA user_version = 1;

CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS project (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_unix_ms INTEGER NOT NULL,
    head_branch TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS branch (
    project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    parent_branch TEXT,
    fork_point_event_id TEXT,
    head_event_id TEXT,
    created_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, name)
);

CREATE TABLE IF NOT EXISTS commit (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    parent_commit_id TEXT,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    raw_blob TEXT NOT NULL,
    created_unix_ms INTEGER NOT NULL,
    UNIQUE(project_id, seq)
);

CREATE TABLE IF NOT EXISTS raw_record (
    id TEXT PRIMARY KEY,
    commit_id TEXT NOT NULL REFERENCES commit(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    slot_key TEXT NOT NULL,
    version INTEGER NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    obs_json TEXT NOT NULL,
    UNIQUE(project_id, slot_key, version)
);
CREATE INDEX IF NOT EXISTS idx_raw_commit ON raw_record(commit_id);
CREATE INDEX IF NOT EXISTS idx_raw_slot ON raw_record(project_id, slot_key);

CREATE TABLE IF NOT EXISTS event (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    branch TEXT NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    at_unix_ms INTEGER NOT NULL,
    parent_id TEXT,
    payload_json TEXT NOT NULL,
    UNIQUE(project_id, branch, seq)
);
CREATE INDEX IF NOT EXISTS idx_event_branch ON event(project_id, branch, seq);

CREATE TABLE IF NOT EXISTS result_set (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    branch TEXT NOT NULL,
    head_event_id TEXT NOT NULL,
    context_hash TEXT NOT NULL,
    computation_json TEXT NOT NULL,
    created_unix_ms INTEGER NOT NULL,
    UNIQUE(project_id, branch, head_event_id)
);

CREATE TABLE IF NOT EXISTS derived (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    result_set_id TEXT NOT NULL REFERENCES result_set(id) ON DELETE CASCADE,
    context_hash TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    source_records_json TEXT NOT NULL,
    algo_version TEXT NOT NULL,
    param_fingerprint TEXT NOT NULL,
    seq_in_set INTEGER NOT NULL,
    PRIMARY KEY (project_id, branch, result_set_id, kind, id)
);
CREATE INDEX IF NOT EXISTS idx_derived_lookup ON derived(project_id, branch, kind, id);

CREATE TABLE IF NOT EXISTS decision (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    branch TEXT NOT NULL,
    seq INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    boundary_id TEXT NOT NULL,
    candidate_id TEXT,
    action TEXT NOT NULL,
    undone INTEGER NOT NULL DEFAULT 0,
    superseded_by TEXT,
    offset_json TEXT,
    created_unix_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_decision_target ON decision(project_id, branch, boundary_id);

CREATE TABLE IF NOT EXISTS kv_blob (
    project_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (project_id, key)
);
