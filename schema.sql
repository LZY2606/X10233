PRAGMA application_id = 0x47534231;

CREATE TABLE IF NOT EXISTS projects (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    created_micros INTEGER NOT NULL,
    marker_name   TEXT NOT NULL DEFAULT '',
    marker_number TEXT NOT NULL DEFAULT '',
    receiver      TEXT NOT NULL DEFAULT '',
    params_json   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS batches (
    id             TEXT PRIMARY KEY,
    project_id     TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    seq            INTEGER NOT NULL,
    label          TEXT NOT NULL DEFAULT '',
    raw_text       TEXT NOT NULL DEFAULT '',
    parent_batch_id TEXT REFERENCES batches(id),
    superseded     INTEGER NOT NULL DEFAULT 0,
    created_via    TEXT NOT NULL DEFAULT 'import',
    created_micros INTEGER NOT NULL DEFAULT 0,
    UNIQUE(project_id, seq)
);

CREATE TABLE IF NOT EXISTS epochs (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    batch_seq  INTEGER NOT NULL,
    micros     INTEGER NOT NULL,
    iso        TEXT NOT NULL,
    UNIQUE(project_id, micros)
);

CREATE TABLE IF NOT EXISTS obs (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    batch_seq  INTEGER NOT NULL,
    epoch_id   TEXT NOT NULL REFERENCES epochs(id) ON DELETE CASCADE,
    micros     INTEGER NOT NULL,
    sat        TEXT NOT NULL,
    code       TEXT NOT NULL,
    band       INTEGER NOT NULL,
    pseudo     TEXT NOT NULL,   -- Qty JSON（单位 m）
    phase      TEXT NOT NULL,   -- Qty JSON（单位 cycle）
    doppler    TEXT NOT NULL,   -- Qty JSON（单位 Hz）
    lli        INTEGER,         -- 原样；无法解析为 NULL
    lli_raw    TEXT NOT NULL,
    snr        TEXT NOT NULL,   -- Qty JSON（单位 dBHz）
    raw_line   INTEGER NOT NULL DEFAULT 0,
    status     TEXT NOT NULL DEFAULT 'active'
);

CREATE TABLE IF NOT EXISTS clock_events (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    batch_seq  INTEGER NOT NULL,
    epoch_id   TEXT NOT NULL REFERENCES epochs(id) ON DELETE CASCADE,
    micros     INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    delta      TEXT NOT NULL,   -- Qty JSON（单位 s）
    note       TEXT NOT NULL DEFAULT '',
    raw_line   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS sat_states (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    batch_seq  INTEGER NOT NULL,
    epoch_id   TEXT NOT NULL REFERENCES epochs(id) ON DELETE CASCADE,
    micros     INTEGER NOT NULL,
    sat        TEXT NOT NULL,
    x          TEXT NOT NULL, y TEXT NOT NULL, z TEXT NOT NULL,
    clock_bias TEXT NOT NULL,
    datum      TEXT NOT NULL DEFAULT 'ITRF2014',
    health     TEXT NOT NULL DEFAULT '',
    raw_line   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS freqs (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    batch_seq    INTEGER NOT NULL,
    sat          TEXT NOT NULL,
    band         INTEGER NOT NULL,
    code         TEXT NOT NULL,
    frequency_hz TEXT NOT NULL,
    raw_line     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS diagnostics (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    batch_seq  INTEGER NOT NULL,
    line_no    INTEGER NOT NULL,
    code       TEXT NOT NULL,
    message    TEXT NOT NULL,
    raw_line   TEXT NOT NULL DEFAULT '',
    fatal      INTEGER NOT NULL DEFAULT 0
);

-- 评审历史（分支与只追加事件）
CREATE TABLE IF NOT EXISTS branches (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    parent_branch_id TEXT REFERENCES branches(id),
    fork_event_id TEXT,
    head_seq    INTEGER NOT NULL DEFAULT -1,
    created_micros INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    branch_id     TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,
    kind          TEXT NOT NULL,
    payload_json  TEXT NOT NULL,
    parent_event_id TEXT,
    created_micros INTEGER NOT NULL,
    UNIQUE(branch_id, seq)
);

-- 运行快照：事件序号上的不可变派生结果
CREATE TABLE IF NOT EXISTS runs (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    branch_id     TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
    event_seq     INTEGER NOT NULL,
    algo_version  TEXT NOT NULL,
    params_fp     TEXT NOT NULL,
    source_batch_seq INTEGER NOT NULL,
    status        TEXT NOT NULL,         -- current | stale
    UNIQUE(branch_id, event_seq)
);

CREATE TABLE IF NOT EXISTS combos (
    id          TEXT PRIMARY KEY,
    run_id      TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    project_id  TEXT NOT NULL,
    epoch_micros INTEGER NOT NULL,
    sat         TEXT NOT NULL,
    combo       TEXT NOT NULL,           -- gf | mw | td | dop
    value       REAL,                    -- 规范化后的数值（带 unit）
    unit        TEXT NOT NULL,
    status      TEXT NOT NULL,           -- ok | UNKNOWN:<原因>
    detail_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(run_id, sat, epoch_micros, combo)
);

CREATE TABLE IF NOT EXISTS arcs (
    id          TEXT PRIMARY KEY,
    run_id      TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    project_id  TEXT NOT NULL,
    sat         TEXT NOT NULL,
    band        INTEGER NOT NULL,
    start_micros INTEGER NOT NULL,
    end_micros   INTEGER NOT NULL,
    end_reason  TEXT NOT NULL,           -- GAP | CYCLE_SLIP | END
    UNIQUE(run_id, sat, band, start_micros)
);

CREATE TABLE IF NOT EXISTS arc_history (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL,
    branch_id   TEXT NOT NULL,
    sat         TEXT NOT NULL,
    band        INTEGER NOT NULL,
    boundary_micros INTEGER NOT NULL,
    reason      TEXT NOT NULL,
    event_id    TEXT NOT NULL,
    created_micros INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS candidates (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL,
    run_id       TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    branch_id    TEXT NOT NULL,
    sat          TEXT NOT NULL,
    epoch_micros INTEGER NOT NULL,
    n_band1      INTEGER,
    n_band2      INTEGER,
    kind         TEXT NOT NULL,          -- slip | lli_alert | clock | ambiguity_family
    score        REAL NOT NULL,
    consensus    TEXT NOT NULL,          -- unanimous | disagreement | ambiguous
    detectors_ok TEXT NOT NULL,          -- JSON 数组
    decision     TEXT NOT NULL DEFAULT 'open', -- open | accepted | rejected
    decided_event_id TEXT,
    reject_reason TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS candidate_detectors (
    id           TEXT PRIMARY KEY,
    candidate_id TEXT NOT NULL REFERENCES candidates(id) ON DELETE CASCADE,
    run_id       TEXT NOT NULL,
    detector     TEXT NOT NULL,          -- gf | mw | td | dop | lli
    supports     INTEGER NOT NULL,       -- 0/1
    residual     REAL,
    unit         TEXT NOT NULL DEFAULT '',
    detail_json  TEXT NOT NULL DEFAULT '{}'
);

-- 机器可读判断依据（检测器分歧、钟跳/周跳分离、修复只存偏移层等）
CREATE TABLE IF NOT EXISTS judgments (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL,
    run_id       TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    sat          TEXT NOT NULL DEFAULT '',
    epoch_micros INTEGER,
    code         TEXT NOT NULL,
    message      TEXT NOT NULL,
    detail_json  TEXT NOT NULL DEFAULT '{}'
);

-- 修复偏移层（绝不回写载波原始值）
CREATE TABLE IF NOT EXISTS offsets (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL,
    branch_id    TEXT NOT NULL,
    event_id     TEXT NOT NULL,
    sat          TEXT NOT NULL,
    band         INTEGER NOT NULL,
    epoch_micros INTEGER NOT NULL,
    n_cycles     INTEGER NOT NULL,
    UNIQUE(branch_id, sat, band, epoch_micros)
);

CREATE TABLE IF NOT EXISTS stale_marks (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL,
    branch_id    TEXT NOT NULL,
    old_run_id   TEXT NOT NULL,
    object_kind  TEXT NOT NULL,          -- combo | arc | candidate
    object_id    TEXT NOT NULL,
    reason       TEXT NOT NULL,          -- SOURCE_CHANGE:<dep...>
    UNIQUE(branch_id, object_kind, object_id)
);

CREATE INDEX IF NOT EXISTS idx_obs_lookup ON obs(project_id, sat, micros, band);
CREATE INDEX IF NOT EXISTS idx_events_branch ON events(branch_id, seq);
CREATE INDEX IF NOT EXISTS idx_runs_branch ON runs(branch_id, event_seq);
CREATE INDEX IF NOT EXISTS idx_combos_run ON combos(run_id);
CREATE INDEX IF NOT EXISTS idx_cand_run ON candidates(run_id);
