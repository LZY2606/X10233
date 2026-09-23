PRAGMA user_version = 1;

CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    meta_json TEXT NOT NULL DEFAULT '{}'
);

-- Raw, immutable input layer ------------------------------------------------
CREATE TABLE IF NOT EXISTS input_versions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    seq INTEGER NOT NULL,
    parent_id TEXT REFERENCES input_versions(id),
    station TEXT NOT NULL,
    raw_json TEXT NOT NULL,
    created_ms INTEGER NOT NULL DEFAULT 0,
    UNIQUE(project_id, seq)
);

CREATE TABLE IF NOT EXISTS epochs (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    input_version_id TEXT NOT NULL REFERENCES input_versions(id),
    seq INTEGER NOT NULL,
    time TEXT NOT NULL,
    flag TEXT NOT NULL,
    raw_json TEXT NOT NULL,
    PRIMARY KEY (id, input_version_id)
);
CREATE INDEX IF NOT EXISTS idx_epochs_iv ON epochs(input_version_id);

CREATE TABLE IF NOT EXISTS epoch_changes (
    input_version_id TEXT NOT NULL REFERENCES input_versions(id),
    epoch_time TEXT NOT NULL,
    change_kind TEXT NOT NULL,
    PRIMARY KEY (input_version_id, epoch_time)
);

CREATE TABLE IF NOT EXISTS diagnostics (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    input_version_id TEXT NOT NULL REFERENCES input_versions(id),
    code TEXT NOT NULL,
    message TEXT NOT NULL,
    time TEXT,
    sat TEXT,
    obs_code TEXT,
    PRIMARY KEY (id, input_version_id)
);

-- Rule layer -----------------------------------------------------------------
CREATE TABLE IF NOT EXISTS rules (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    seq INTEGER NOT NULL,
    fingerprint TEXT NOT NULL,
    algorithm_version TEXT NOT NULL,
    params_json TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT ''
);

-- Branch / event log ---------------------------------------------------------
CREATE TABLE IF NOT EXISTS nodes (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    seq INTEGER NOT NULL,
    parent_node_id TEXT REFERENCES nodes(id),
    branch_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    input_version_id TEXT REFERENCES input_versions(id),
    rule_id TEXT REFERENCES rules(id),
    payload_json TEXT NOT NULL,
    UNIQUE(project_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_nodes_branch ON nodes(project_id, branch_id);

-- Derived layer --------------------------------------------------------------
CREATE TABLE IF NOT EXISTS results (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    node_id TEXT NOT NULL REFERENCES nodes(id),
    branch_id TEXT NOT NULL,
    input_version_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    accepted_fp TEXT NOT NULL,
    json TEXT NOT NULL,
    PRIMARY KEY (id, node_id)
);
CREATE INDEX IF NOT EXISTS idx_results_branch ON results(project_id, branch_id);

-- Repair offset layer only; raw observations are never rewritten -------------
CREATE TABLE IF NOT EXISTS repairs (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    branch_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    n1 INTEGER NOT NULL,
    n2 INTEGER NOT NULL,
    node_id TEXT NOT NULL,
    active INTEGER NOT NULL,
    offset_note TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_repairs_branch ON repairs(project_id, branch_id);

-- Export/import bookkeeping (identity preservation audit) --------------------
CREATE TABLE IF NOT EXISTS bundles (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    exported_ms INTEGER NOT NULL,
    sha_fingerprint TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT ''
);
