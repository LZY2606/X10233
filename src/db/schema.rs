pub const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS project (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_event_id TEXT,
    created_ms INTEGER NOT NULL,
    algo_version TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS branch (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES project(id),
    name TEXT NOT NULL,
    parent_branch_id TEXT,
    fork_from_event_id TEXT,
    created_ms INTEGER NOT NULL,
    head_event_id TEXT,
    head_seq INTEGER NOT NULL DEFAULT 0,
    UNIQUE(project_id, name)
);

CREATE TABLE IF NOT EXISTS source_set (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES project(id),
    parent_source_id TEXT,
    created_event_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    obs_text TEXT,
    clock_text TEXT,
    sat_state_text TEXT,
    bundle_json TEXT NOT NULL,
    diagnostics_json TEXT NOT NULL,
    reviewer TEXT NOT NULL,
    reason TEXT NOT NULL,
    created_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS param_set (
    fingerprint TEXT PRIMARY KEY,
    params_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS event (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES project(id),
    branch_id TEXT NOT NULL REFERENCES branch(id),
    seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    source_id TEXT,
    param_fingerprint TEXT,
    decision_key TEXT,
    supersedes_event_id TEXT,
    undoes_event_id TEXT,
    undone INTEGER NOT NULL DEFAULT 0,
    payload_json TEXT NOT NULL,
    author TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    UNIQUE(branch_id, seq)
);

CREATE TABLE IF NOT EXISTS derived_node (
    id TEXT NOT NULL,
    branch_id TEXT NOT NULL REFERENCES branch(id),
    node_type TEXT NOT NULL,
    content_json TEXT NOT NULL,
    created_event_seq INTEGER NOT NULL,
    last_seen_event_seq INTEGER NOT NULL,
    superseded_event_seq INTEGER,
    sat TEXT,
    epoch_index INTEGER,
    PRIMARY KEY (id, branch_id)
);

CREATE INDEX IF NOT EXISTS idx_node_branch_type ON derived_node(branch_id, node_type);
CREATE INDEX IF NOT EXISTS idx_event_branch ON event(branch_id, seq);
"#;
