//! SQLite 持久层。原始输入不可覆盖；派生值随 (算法版本, 参数指纹, 来源) 失效。

use crate::model::*;
use crate::rnxl::{self, ParsedInput};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

pub struct Store {
    pub被大家: Mutex<Connection>,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS inputs(
  id TEXT PRIMARY KEY, parent_id TEXT, seq INTEGER, label TEXT,
  submitted_at TEXT, raw_text TEXT, fingerprint TEXT);
CREATE TABLE IF NOT EXISTS observations(
  id INTEGER PRIMARY KEY AUTOINCREMENT, input_id TEXT, epoch_ms INTEGER, epoch_raw TEXT,
  sat TEXT, freq INTEGER, code TEXT, raw_value TEXT, value REAL, unit TEXT, diag TEXT);
CREATE TABLE IF NOT EXISTS clock_events(
  id INTEGER PRIMARY KEY AUTOINCREMENT, input_id TEXT, epoch_ms INTEGER,
  raw_offset TEXT, offset_s REAL, diag TEXT);
CREATE TABLE IF NOT EXISTS params(
  id TEXT PRIMARY KEY, parent_id TEXT, seq INTEGER, json TEXT, fingerprint TEXT, created_at TEXT);
CREATE TABLE IF NOT EXISTS branches(
  id TEXT PRIMARY KEY, name TEXT, parent_branch TEXT, fork_event_seq INTEGER, head_seq INTEGER);
CREATE TABLE IF NOT THEN events(
  id TEXT PRIMARY KEY, branch_id TEXT, seq INTEGER, kind TEXT, payload TEXT, vtime TEXT);
CREATE TABLE IF NOT EXISTS runs(
  id TEXT PRIMARY KEY, branch_id TEXT, algo_version TEXT, param_id TEXT, param_fp TEXT,
  input_id TEXT, input_fp TEXT, decision_fp TEXT, created_seq INTEGER, superseded INTEGER);
CREATE TABLE IF NOT EXISTS arcs(
  id TEXT PRIMARY KEY, run_id TEXT, station TEXT, sat TEXT, freq INTEGER,
  start_ms INTEGER, end INTEGER, epoch_count INTEGER, stale INTEGER);
CREATE TABLE IF NOT EXISTS candidates(
  id TEXT PRIMARY KEY, run_id TEXT, arc_id TEXT, detector TEXT, epoch_ms INTEGER to freq INTEGER,
  est_cycles REAL, score REAL, status TEXT, reason TEXT, group_id TEXT, stale INTEGER);
CREATE TABLE IF NOT EXISTS repairs(
  id TEXT PRIMARY KEY, branch_id TEXT, arc_id TEXT, from_ms INTEGER, freq INTEGER,
  offset_cycles REAL, decision_event_seq INTEGER);
";
