-- Local-first GNSS cycle-slip workbench schema.
-- Raw layer is immutable; revisions are new versions with parent links.
-- Derived layer carries algorithm version, ruleset fingerprint and direct sources.

CREATE TABLE IF NOT EXISTS projects (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    logical_clock   INTEGER NOT NULL,
    head_event_id   TEXT,
    created_seq     INTEGER NOT NULL DEFAULT 0
);

-- Append-only event log. Every human action and every system action is here.
CREATE TABLE IF NOT EXISTS events (
    project_id      TEXT NOT NULL REFERENCES projects(id),
    seq             INTEGER NOT NULL,
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,
    basis_event_id  TEXT,
    manifest_id     TEXT,
    ruleset_id      TEXT,
    payload_json    TEXT NOT NULL,
    UNIQUE (project_id, seq)
);

-- Input manifest versions (immutable content-addressed snapshots).
CREATE TABLE IF NOT EXISTS manifests (
    project_id      TEXT NOT NULL REFERENCES projects(id),
    id              TEXT PRIMARY KEY,
    seq             INTEGER NOT NULL,
    label           TEXT NOT NULL,
    parent_id       TEXT,
    content_json    TEXT NOT NULL,
    content_hash    TEXT NOT NULL,
    UNIQUE (project_id, seq)
);

-- Deduplicated raw content items, keyed by their content hash.
CREATE TABLE IF NOT EXISTS raw_items (
    content_hash    TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,
    content_json    TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS manifest_items (
    manifest_id     TEXT NOT NULL REFERENCES manifests(id),
    kind            TEXT NOT NULL,
    key             TEXT NOT NULL,
    content_hash    TEXT NOT NULL REFERENCES raw_items(content_hash),
    ord             INTEGER NOT NULL,
    PRIMARY KEY (manifest_id, kind, key)
);

-- Ruleset versions.
CREATE TABLE IF NOT EXISTS rulesets (
    project_id      TEXT NOT NULL REFERENCES projects(id),
    id              TEXT PRIMARY KEY,
    seq             INTEGER NOT NULL,
    name            TEXT NOT NULL,
    parent_id       TEXT,
    fingerprint     TEXT NOT NULL,
    content_json    TEXT NOT NULL
);

-- Engine run output. One batch per manifest/ruleset/algorithm combination.
CREATE TABLE IF NOT EXISTS derived_batches (
    project_id      TEXT NOT NULL REFERENCES projects(id),
    id              TEXT PRIMARY KEY,
    manifest_id     TEXT NOT NULL,
    ruleset_id      TEXT NOT NULL,
    algorithm       TEXT NOT NULL,
    created_event_id TEXT NOT NULL,
    content_json    TEXT NOT NULL
);

-- Per adjacent-epoch-pair, per-satellite checks (GF / MW / TD / LLI / clock).
CREATE TABLE IF NOT EXISTS epoch_checks (
    batch_id        TEXT NOT NULL REFERENCES derived_batches(id),
    id              TEXT PRIMARY KEY,
    site            TEXT NOT NULL,
    sat             TEXT NOT NULL,
    epoch_seq_a     INTEGER NOT NULL,
    epoch_seq_b     INTEGER NOT NULL,
    gap_seconds     REAL,
    status          TEXT NOT NULL,
    content_json    TEXT NOT NULL,
    UNIQUE (batch_id, sat, epoch_seq_a, epoch_seq_b)
);

-- Integer ambiguity-change hypotheses from each detector.
CREATE TABLE IF NOT EXISTS candidates (
    batch_id        TEXT NOT NULL REFERENCES derived_batches(id),
    id              TEXT PRIMARY KEY,
    check_id        TEXT NOT NULL REFERENCES epoch_checks(id),
    sat             TEXT NOT NULL,
    epoch_seq       INTEGER NOT NULL,
    n1              INTEGER,
    n2              INTEGER,
    gf_support      TEXT NOT NULL,
    mw_support      TEXT NOT NULL,
    td_support      TEXT NOT NULL,
    lli_note        TEXT NOT NULL,
    score           REAL NOT NULL,
    rank            INTEGER NOT NULL,
    status          TEXT NOT NULL, -- pending | accepted | rejected | superseded
    reject_reason   TEXT,
    content_json    TEXT NOT NULL,
    UNIQUE (batch_id, check_id, n1, n2)
);

-- Station-satellite-frequency arc pieces. Identities are content-addressed to
-- the (sat, frequency, start epoch, parent arc), so unrelated local edits do
-- not change an arc identity.
CREATE TABLE IF NOT EXISTS arcs (
    batch_id        TEXT NOT NULL REFERENCES derived_batches(id),
    id              TEXT NOT NULL,
    site            TEXT NOT NULL,
    sat             TEXT NOT NULL,
    frequency_no    INTEGER NOT NULL,
    start_epoch_seq INTEGER NOT NULL,
    end_epoch_seq   INTEGER NOT NULL,
    parent_arc_id   TEXT,
    boundary_reason TEXT NOT NULL,
    PRIMARY KEY (batch_id, id)
);

-- Human decisions. Repair lives ONLY at the offset layer; raw rows untouched.
CREATE TABLE IF NOT EXISTS decisions (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id),
    event_id        TEXT NOT NULL REFERENCES events(id),
    candidate_id    TEXT,
    sat             TEXT NOT NULL,
    epoch_seq       INTEGER NOT NULL,
    action          TEXT NOT NULL, -- accept | reject | adjust
    n1              INTEGER,
    n2              INTEGER,
    note            TEXT NOT NULL,
    conflict_with   TEXT
);

CREATE TABLE IF NOT EXISTS phase_offsets (
    project_id      TEXT NOT NULL REFERENCES projects(id),
    event_id        TEXT NOT NULL REFERENCES events(id),
    sat             TEXT NOT NULL,
    epoch_seq       INTEGER NOT NULL,
    frequency_no    INTEGER NOT NULL,
    cycles          INTEGER NOT NULL,
    layer           TEXT NOT NULL DEFAULT 'phase_ambiguity',
    PRIMARY KEY (project_id, sat, epoch_seq, frequency_no)
);

CREATE TABLE IF NOT EXISTS clock_offsets (
    project_id      TEXT NOT NULL REFERENCES projects(id),
    event_id        TEXT NOT NULL REFERENCES events(id),
    epoch_seq       INTEGER NOT NULL,
    meters          REAL NOT NULL,
    layer           TEXT NOT NULL DEFAULT 'receiver_clock',
    PRIMARY KEY (project_id, epoch_seq)
);

CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id, seq);
CREATE INDEX IF NOT EXISTS idx_checks_batch ON epoch_checks(batch_id);
CREATE INDEX IF NOT EXISTS idx_candidates_check ON candidates(check_id);
CREATE INDEX IF NOT EXISTS idx_arcs_batch ON arcs(batch_id);
CREATE INDEX IF NOT EXISTS idx_decisions_project ON decisions(project_id);
