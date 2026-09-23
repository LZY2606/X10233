// SQLite schema for the carrier-break workbench.
//
// Design summary
// ---------------
// * Raw inputs are append-only (`submission` + `raw_line` + `observation`).
//   They are never overwritten; a corrected source arrives as a new submission
//   with parent relationships and may supersede specific (sat,epoch,band) keys.
// * Every derived object (run / arc / residual / candidate) carries an
//   `algo_version`, a `param_fingerprint` and a `source_key`, so a displayed
//   value can always be traced back to its direct inputs, rule version and the
//   human decision in effect.
// * Runs are immutable snapshots on a branch up to an event sequence number.
//   Re-opening an old event restores that run; later recomputation never
//   overwrites history.
// * Identities are stable strings assigned at creation and preserved verbatim
//   across export/import.

pub const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS project (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    seed        INTEGER NOT NULL,
    algo_version TEXT NOT NULL
);

-- One raw import (initial or later amendment). Immutable.
CREATE TABLE IF NOT EXISTS submission (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL REFERENCES project(id),
    kind         TEXT NOT NULL,           -- import | amend_source
    label        TEXT NOT NULL,
    parent_id    TEXT,                    -- prior submission this builds on
    mode         TEXT,                    -- null=full | supersede_direct_source
    target_sat   TEXT,
    target_epoch INTEGER,
    reason       TEXT,
    content_hash TEXT NOT NULL,
    received_at  TEXT NOT NULL
);

-- Every non-empty input line kept verbatim (comments included for audit).
CREATE TABLE IF NOT EXISTS raw_line (
    id            TEXT PRIMARY KEY,
    submission_id TEXT NOT NULL REFERENCES submission(id),
    line_no       INTEGER NOT NULL,
    raw_text      TEXT NOT NULL,
    directive     TEXT,                  -- STATION|DATUM|FREQ|EPOCH|OBS|CLOCKJUMP|AMEND|#
    parse_status  TEXT NOT NULL DEFAULT 'ok',  -- ok | error
    diagnostic    TEXT
);

CREATE TABLE IF NOT EXISTS epoch_rec (
    id        TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES project(id),
    idx       INTEGER NOT NULL,
    t_seconds REAL NOT NULL,
    iso_tag   TEXT NOT NULL,             -- original representation preserved
    rcvr_clk_s TEXT NOT NULL,            -- original representation preserved
    clock_event_flag INTEGER NOT NULL,
    UNIQUE(project_id, idx)
);

-- Frequency basis / observation code map, per constellation+band, verbatim.
CREATE TABLE IF NOT EXISTS frequency_basis (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL REFERENCES project(id),
    gnss         TEXT NOT NULL,          -- G | R | ...
    band         TEXT NOT NULL,          -- L1 | L2
    freq_hz      REAL NOT NULL,
    wavelength_m REAL NOT NULL,
    freq_number  INTEGER,                -- GLONASS k, else null
    codes_raw    TEXT NOT NULL,          -- original codes= token
    UNIQUE(project_id, gnss, band)
);

CREATE TABLE IF NOT EXISTS header_attr (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL REFERENCES project(id),
    kind         TEXT NOT NULL,          -- STATION | DATUM
    key          TEXT NOT NULL,
    value_raw    TEXT NOT NULL
);

-- Effective per-epoch observation keyed by (project, sat, epoch, band).
-- `current_*` points at the winning submission; superseded rows remain.
CREATE TABLE IF NOT EXISTS observation (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL REFERENCES project(id),
    submission_id TEXT NOT NULL REFERENCES submission(id),
    line_id       TEXT NOT NULL REFERENCES raw_line(id),
    epoch_idx     INTEGER NOT NULL,
    sat           TEXT NOT NULL,
    gnss          TEXT NOT NULL,
    band          TEXT NOT NULL,
    code_phase    TEXT NOT NULL,          -- L1/L2 original token (cycle)
    code_pseudo   TEXT,                   -- P1/P2 original token (metre)
    code_doppler  TEXT,                   -- D1/D2 original token (hertz), null if absent
    code_snr      TEXT,                   -- S1/S2 original token (rinex_ssi)
    code_lli      TEXT,                   -- LLI original token (bits)
    phase_cy      REAL,
    pseudo_m      REAL,
    doppler_hz    REAL,
    lli_bits      INTEGER,
    value_status  TEXT NOT NULL DEFAULT 'ok',  -- ok | unconvertible
    value_diag    TEXT,
    is_current    INTEGER NOT NULL DEFAULT 1,
    superseded_by TEXT
);
CREATE INDEX IF NOT EXISTS idx_obs_key
    ON observation(project_id, sat, epoch_idx, band, is_current);

CREATE TABLE IF NOT EXISTS clock_event (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL REFERENCES project(id),
    submission_id TEXT NOT NULL REFERENCES submission(id),
    epoch_idx     INTEGER NOT NULL,
    delta_s_raw   TEXT NOT NULL,
    scope_raw     TEXT NOT NULL,
    cause_raw     TEXT NOT NULL
);

-- Parameter sets, versioned. A fingerprint lets derived rows pin the rules used.
CREATE TABLE IF NOT EXISTS param_set (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL REFERENCES project(id),
    version_no    INTEGER NOT NULL,
    parent_id     TEXT,
    label         TEXT NOT NULL,
    fingerprint   TEXT NOT NULL,
    params_json   TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    UNIQUE(project_id, version_no)
);

-- Append-only audit/decision timeline.
CREATE TABLE IF NOT EXISTS event (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL REFERENCES project(id),
    branch_id    TEXT NOT NULL,
    seq          INTEGER NOT NULL,
    kind         TEXT NOT NULL,   -- import|amend_source|revise_params|fork|decision|undo
    label        TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_virtual_t TEXT NOT NULL,
    UNIQUE(branch_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_event_project ON event(project_id, seq);

CREATE TABLE IF NOT EXISTS branch (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL REFERENCES project(id),
    name         TEXT NOT NULL,
    parent_branch_id TEXT,
    fork_after_seq   INTEGER NOT NULL DEFAULT 0,
    created_event_id TEXT,
    UNIQUE(project_id, name)
);

-- Immutable derived snapshot on a branch up to an event sequence.
CREATE TABLE IF NOT EXISTS run (
    id             TEXT PRIMARY KEY,
    project_id     TEXT NOT NULL REFERENCES project(id),
    branch_id      TEXT NOT NULL REFERENCES branch(id),
    up_to_seq      INTEGER NOT NULL,
    algo_version   TEXT NOT NULL,
    param_fingerprint TEXT NOT NULL,
    input_fingerprint TEXT NOT NULL,
    created_virtual_t TEXT NOT NULL,
    note           TEXT,
    UNIQUE(branch_id, up_to_seq)
);

CREATE TABLE IF NOT EXISTS arc (
    id          TEXT PRIMARY KEY,
    run_id      TEXT NOT NULL REFERENCES run(id),
    project_id  TEXT NOT NULL REFERENCES project(id),
    sat         TEXT NOT NULL,
    band        TEXT NOT NULL,
    start_epoch INTEGER NOT NULL,
    end_epoch   INTEGER NOT NULL,
    reason      TEXT NOT NULL,   -- initial|gap|missing_frequency|accepted_slip
    source_key  TEXT NOT NULL,   -- input identity this arc depends on
    status      TEXT NOT NULL DEFAULT 'current'  -- current|stale
);
CREATE INDEX IF NOT EXISTS idx_arc_run ON arc(run_id);

-- Multi-frequency / detector residuals.
CREATE TABLE IF NOT EXISTS residual (
    id          TEXT PRIMARY KEY,
    run_id      TEXT NOT NULL REFERENCES run(id),
    project_id  TEXT NOT NULL REFERENCES project(id),
    sat         TEXT NOT NULL,
    band        TEXT,
    epoch_idx   INTEGER NOT NULL,
    detector    TEXT NOT NULL,   -- gf(ionosphere)|mw(melbourne_wubbena)|td(time_difference)
    value       REAL,
    unit        TEXT NOT NULL,
    status      TEXT NOT NULL,   -- ok|unknown|missing_pair
    freshness   TEXT NOT NULL DEFAULT 'current', -- current|stale (local expiry)
    detail_json TEXT NOT NULL,
    source_key  TEXT NOT NULL,
    algo_version TEXT NOT NULL,
    param_fingerprint TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_resid_run ON residual(run_id);

-- Cycle-slip hypotheses/candidates: accepted and rejected coexist.
CREATE TABLE IF NOT EXISTS candidate (
    id           TEXT PRIMARY KEY,
    run_id       TEXT NOT NULL REFERENCES run(id),
    project_id   TEXT NOT NULL REFERENCES project(id),
    sat          TEXT NOT NULL,
    epoch_idx    INTEGER NOT NULL,
    n1           INTEGER,
    n2           INTEGER,
    kind         TEXT NOT NULL,   -- slip|clock|lli_false_positive
    gf_error_m   REAL,
    mw_error_cy  REAL,
    td_error_cy  REAL,
    score        REAL NOT NULL,
    detector_votes_json TEXT NOT NULL,
    status       TEXT NOT NULL,   -- proposed|accepted|rejected|superseded|ambiguous
    reject_reason TEXT,
    freshness   TEXT NOT NULL DEFAULT 'current', -- current|stale (local expiry)
    source_key   TEXT NOT NULL,
    algo_version TEXT NOT NULL,
    param_fingerprint TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cand_run ON candidate(run_id);

-- Repair exists only as an offset layer; raw observations are never rewritten.
CREATE TABLE IF NOT EXISTS offset_layer (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL REFERENCES project(id),
    run_id       TEXT REFERENCES run(id),
    sat          TEXT NOT NULL,
    epoch_idx    INTEGER NOT NULL,
    band         TEXT NOT NULL,
    offset_cy    INTEGER NOT NULL,
    decision_event_id TEXT,
    applies_from_run TEXT
);

-- Persistent, machine-readable judgements backing the narrative.
CREATE TABLE IF NOT EXISTS judgement (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES project(id),
    event_id    TEXT REFERENCES event(id),
    code        TEXT NOT NULL,
    subject_key TEXT NOT NULL,
    detail_json TEXT NOT NULL
);
#END
