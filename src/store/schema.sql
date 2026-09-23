-- Carrier-break station: append-only, local-first SQLite schema.
-- Raw inputs are immutable once committed; revisions are new versions with a
-- parent pointer. Derived rows carry algorithm/parameter fingerprints and
-- direct sources. Nothing is ever hard-deleted from the event log.

CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS imports (
  import_id      TEXT PRIMARY KEY,
  seq            INTEGER NOT NULL UNIQUE,
  parent_import  TEXT REFERENCES imports(import_id),
  kind           TEXT NOT NULL,            -- base | observation_revision | parameter_revision
  case_id        TEXT NOT NULL,
  title          TEXT NOT NULL,
  created_ms     INTEGER NOT NULL,
  source_kind    TEXT NOT NULL,            -- text | manual
  content_sha256 TEXT NOT NULL,
  note           TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS station (
  import_id TEXT PRIMARY KEY REFERENCES imports(import_id),
  code TEXT NOT NULL, name TEXT NOT NULL, marker TEXT NOT NULL,
  x_m REAL NOT NULL, y_m REAL NOT NULL, z_m REAL NOT NULL,
  coord_datum TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS signal (
  import_id TEXT NOT NULL REFERENCES imports(import_id),
  band INTEGER NOT NULL,
  freq_number TEXT NOT NULL,
  code_p TEXT NOT NULL, code_l TEXT NOT NULL,
  code_d TEXT NOT NULL, code_s TEXT NOT NULL,
  frequency_hz REAL NOT NULL, wavelength_m REAL NOT NULL,
  PRIMARY KEY (import_id, band)
);

CREATE TABLE IF NOT EXISTS epoch (
  epoch_id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL UNIQUE,
  import_id TEXT NOT NULL REFERENCES imports(import_id),
  time_datum TEXT NOT NULL,
  gps_seconds REAL NOT NULL,
  ymdhms TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sat_state (
  epoch_id TEXT NOT NULL REFERENCES epoch(epoch_id),
  sat TEXT NOT NULL,
  x_m REAL, y_m REAL, z_m REAL,
  coord_datum TEXT NOT NULL DEFAULT 'WGS84/ECEF-m',
  clock_bias_s REAL,
  clock_bias_original TEXT,
  health TEXT NOT NULL DEFAULT '',
  PRIMARY KEY (epoch_id, sat)
);

CREATE TABLE IF NOT EXISTS clock_event (
  epoch_id TEXT PRIMARY KEY REFERENCES epoch(epoch_id),
  import_id TEXT NOT NULL REFERENCES imports(import_id),
  kind TEXT NOT NULL,
  magnitude_s REAL,
  note TEXT NOT NULL
);

-- Versioned raw observation cells (immutable rows).
CREATE TABLE IF NOT EXISTS obs_value (
  version_id TEXT PRIMARY KEY,
  import_id TEXT NOT NULL REFERENCES imports(import_id),
  parent_version_id TEXT REFERENCES obs_value(version_id),
  epoch_id TEXT NOT NULL REFERENCES epoch(epoch_id),
  sat TEXT NOT NULL,
  band INTEGER NOT NULL,
  field TEXT NOT NULL,              -- C1C | L1C | D1C | S1C
  original TEXT NOT NULL,          -- token as imported
  numeric REAL,                    -- NULL when not convertible
  unit TEXT NOT NULL,
  diagnostic TEXT,
  seq INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_obs_lookup ON obs_value(epoch_id, sat, band, field);

-- LLI per sat/band/epoch (versioned through imports).
CREATE TABLE IF NOT EXISTS obs_lli (
  import_id TEXT NOT NULL REFERENCES imports(import_id),
  epoch_id TEXT NOT NULL REFERENCES epoch(epoch_id),
  sat TEXT NOT NULL,
  band INTEGER NOT NULL,
  lli INTEGER NOT NULL,
  PRIMARY KEY (import_id, epoch_id, sat, band)
);

CREATE TABLE IF NOT EXISTS params (
  version_id TEXT PRIMARY KEY,
  import_id TEXT NOT NULL UNIQUE REFERENCES imports(import_id),
  parent_version_id TEXT REFERENCES params(version_id),
  fingerprint TEXT NOT NULL,
  payload TEXT NOT NULL
);

-- Branch / event log (append-only).
CREATE TABLE IF NOT EXISTS branch (
  branch_id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL UNIQUE,
  parent_branch_id TEXT REFERENCES branch(branch_id),
  name TEXT NOT NULL,
  forked_from_event TEXT,
  created_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS event (
  event_id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL REFERENCES branch(branch_id),
  seq INTEGER NOT NULL,
  parent_event_id TEXT REFERENCES event(event_id),
  event_type TEXT NOT NULL,
  at_ms INTEGER NOT NULL,
  import_id TEXT REFERENCES imports(import_id),
  cset_id TEXT,
  payload TEXT NOT NULL,
  UNIQUE (branch_id, seq)
);

-- Derived compute sets (immutable; recomputation appends a new set).
CREATE TABLE IF NOT EXISTS compute_set (
  cset_id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL REFERENCES branch(branch_id),
  seq INTEGER NOT NULL,
  event_id TEXT NOT NULL,
  import_id TEXT NOT NULL REFERENCES imports(import_id),
  params_version TEXT NOT NULL,
  input_fp TEXT NOT NULL,
  algo_version TEXT NOT NULL,
  created_ms INTEGER NOT NULL,
  status TEXT NOT NULL,            -- current | stale | historical
  superseded_by TEXT,
  change_scope TEXT NOT NULL,      -- full | local
  changed_refs TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE IF NOT EXISTS arc (
  cset_id TEXT NOT NULL REFERENCES compute_set(cset_id),
  stable_arc_id TEXT NOT NULL,     -- sat-ARn, stable across recomputations
  sat TEXT NOT NULL,
  band_scope TEXT NOT NULL,
  start_epoch TEXT NOT NULL,
  end_epoch TEXT NOT NULL,
  length INTEGER NOT NULL,
  boundaries TEXT NOT NULL,
  PRIMARY KEY (cset_id, stable_arc_id)
);

CREATE TABLE IF NOT EXISTS residual (
  cset_id TEXT NOT NULL REFERENCES compute_set(cset_id),
  epoch_id TEXT NOT NULL,
  sat TEXT NOT NULL,
  arc_id TEXT NOT NULL,
  gf_m REAL, gf_delta_m REAL,
  mw_wl_cycle REAL, mw_delta_cycle REAL,
  td_l1_cycle REAL, td_l2_cycle REAL,
  doppler_l1_hz REAL, doppler_l2_hz REAL,
  seq INTEGER NOT NULL,
  PRIMARY KEY (cset_id, sat, epoch_id)
);

CREATE TABLE IF NOT EXISTS candidate (
  cset_id TEXT NOT NULL REFERENCES compute_set(cset_id),
  candidate_id TEXT NOT NULL,      -- sat-EPnn or CLK-EPnn, stable across recomputations
  sat TEXT NOT NULL,
  epoch_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  triggers TEXT NOT NULL,
  gf_jump_m REAL, mw_jump_cycle REAL,
  td_l1_cycle REAL, td_l2_cycle REAL,
  gf_predicted_m REAL, mw_predicted_cycle REAL,
  ambiguity INTEGER NOT NULL DEFAULT 0,
  ambiguity_tie_count INTEGER NOT NULL DEFAULT 0,
  note TEXT NOT NULL DEFAULT '',
  members TEXT NOT NULL DEFAULT '[]',
  evidence TEXT NOT NULL DEFAULT '[]',
  hypotheses TEXT NOT NULL DEFAULT '[]',
  engine_status TEXT NOT NULL,     -- open | ambiguous
  PRIMARY KEY (cset_id, candidate_id)
);

-- Human decisions and the offset-only repair layer.
CREATE TABLE IF NOT EXISTS decision (
  decision_id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL REFERENCES branch(branch_id),
  event_id TEXT NOT NULL,
  cset_id TEXT NOT NULL,
  candidate_id TEXT NOT NULL,
  verdict TEXT NOT NULL,           -- accept | reject
  n1 INTEGER, n2 INTEGER,
  rationale TEXT NOT NULL DEFAULT '',
  conflict_with TEXT,
  created_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS repair (
  repair_id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL REFERENCES branch(branch_id),
  decision_id TEXT NOT NULL REFERENCES decision(decision_id),
  cset_id TEXT NOT NULL,
  candidate_id TEXT NOT NULL,
  sat TEXT NOT NULL,
  start_epoch TEXT NOT NULL,
  n1 INTEGER NOT NULL, n2 INTEGER,
  layer TEXT NOT NULL DEFAULT 'offset',
  created_ms INTEGER NOT NULL
);

-- Provenance edges for derived objects.
CREATE TABLE IF NOT EXISTS dependency (
  cset_id TEXT NOT NULL REFERENCES compute_set(cset_id),
  object_id TEXT NOT NULL,         -- arc/candidate stable id
  object_kind TEXT NOT NULL,       -- arc | candidate
  src_kind TEXT NOT NULL,          -- obs_value | epoch | signal | clock_event | params
  src_ref TEXT NOT NULL,           -- version_id / epoch_id / band / epoch_id / params version
  algo_version TEXT NOT NULL,
  params_fp TEXT NOT NULL,
  PRIMARY KEY (cset_id, object_id, src_kind, src_ref)
);

CREATE INDEX IF NOT EXISTS idx_dep ON dependency(cset_id, object_id);
CREATE INDEX IF NOT EXISTS idx_cset_branch ON compute_set(branch_id);
CREATE INDEX IF NOT EXISTS idx_event_branch ON event(branch_id, seq);
CREATE INDEX IF NOT EXISTS idx_decision_branch ON decision(branch_id);
