//! Application service: event-sourced project ledger over the SQLite store.
//!
//! Every human action (import, parameter revision, local source change,
//! accept/reject, undo, conflict, fork) appends a node. Reopening an old event
//! restores the exact dependency versions and visible conclusions: results are
//! never overwritten, they are superseded.

use crate::analysis::{AnalysisOutput, Candidate};
use crate::clock::Clock;
use crate::import::parse_import;
use crate::model::EpochRecord;
use crate::params::Params;
use crate::store::{
    link_input_parent, next_seq, record_removed_epochs, upsert_rule, write_input_snapshot, Store,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAIN_BRANCH: &str = "br-main";

#[derive(Clone)]
pub struct AppService {
    pub store: std::sync::Arc<Store>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub receiver: String,
    pub antenna: String,
    pub station_datum: String,
    pub station_x_m: f64,
    pub station_y_m: f64,
    pub station_z_m: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DecisionPayload {
    pub candidate_id: String,
    pub action: String, // accept | reject
    pub n1: i64,
    pub n2: i64,
    pub rationale: String,
    /// When rejecting/accepting on a branch where the edge was already decided
    /// differently, this names the conflicting branch (for the conflict ledger).
    pub conflicts_with_branch: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodeView {
    pub id: String,
    pub seq: i64,
    pub parent_node_id: Option<String>,
    pub branch_id: String,
    pub kind: String,
    pub created_ms: i64,
    pub input_version_id: Option<String>,
    pub rule_id: Option<String>,
    pub payload: serde_json::Value,
    /// Summary of the visible conclusion at this node (for decision/analysis
    /// nodes), enough to render history without recomputing.
    pub summary: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepairView {
    pub id: String,
    pub candidate_id: String,
    pub n1: i64,
    pub n2: i64,
    pub active: bool,
    pub node_id: String,
    pub offset_note: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StalenessTag {
    pub state: String, // current | stale
    pub reason: String,
    pub stale_against_input: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BranchView {
    pub branch_id: String,
    pub tip_node_id: String,
    pub head_seq: i64,
    pub input_version_id: String,
    pub rule_id: String,
    pub result_node_id: String,
    pub result_id: String,
    pub result_state: StalenessTag,
    pub created_from_fork: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StateView {
    pub project_id: String,
    pub name: String,
    pub current_branch: String,
    pub branches: Vec<BranchView>,
    pub nodes: Vec<NodeView>,
    pub result: serde_json::Value,
    pub raw_epochs: Vec<EpochRecord>,
    pub repairs: Vec<RepairView>,
    pub diagnostics: Vec<serde_json::Value>,
    pub params: Params,
}

fn now(store: &Store) -> i64 {
    store.clock.now_ms()
}

fn default_rule_id(project_id: &str) -> String {
    crate::id::id("rule", &[project_id, "default"])
}

fn initial_input_id(project_id: &str) -> String {
    crate::id::id("iv", &[project_id, "v1"])
}

fn initial_result_node(project_id: &str) -> String {
    crate::id::id("node", &[project_id, MAIN_BRANCH, "0", "initial-analysis"])
}

impl AppService {
    pub fn new(store: std::sync::Arc<Store>) -> Self {
        AppService { store }
    }

    pub fn create_project(&self, name: &str, meta: ProjectMeta) -> rusqlite::Result<String> {
        let c = self.store.conn.lock().unwrap();
        let n: i64 = c.query_row(
            "SELECT COALESCE(MAX(CAST(substr(id, 5) AS INTEGER)),0)+1 FROM projects",
            [],
            |r| r.get(0),
        ).unwrap_or(1);
        let id = format!("prj-{n:04}");
        c.execute(
            "INSERT INTO projects (id, name, created_ms, meta_json) VALUES (?1,?2,?3,?4)",
            rusqlite::params![
                id,
                name,
                now(&self.store),
                serde_json::to_string(&meta).unwrap()
            ],
        )?;
        Ok(id)
    }

    /// Initial immutable import + default rules + first analysis node.
    pub fn import_initial(
        &self,
        project_id: &str,
        raw_text: &str,
    ) -> Result<StateView, String> {
        let (payload, diags) = parse_import(raw_text)?;
        let iv = initial_input_id(project_id);
        let rule = default_rule_id(project_id);
        let params = Params::default();
        let c = self.store.conn.lock().unwrap();
        let tx = c;
        let seq = next_seq(&tx, project_id, "input_versions", "seq")
            .map_err(|e| e.to_string())?;
        write_input_snapshot(
            &tx, project_id, &iv, seq, &payload.station, raw_text, &payload.epochs, &diags,
        ).map_err(|e| e.to_string())?;
        let rseq = next_seq(&tx, project_id, "rules", "seq").map_err(|e| e.to_string())?;
        upsert_rule(&tx, project_id, &rule, rseq, &params, "initial default parameters")
            .map_err(|e| e.to_string())?;

        let import_node = crate::id::id("node", &[project_id, MAIN_BRANCH, "1", "import"]);
        let analysis_node = initial_result_node(project_id);
        let ts = now(&self.store);
        tx.execute(
            "INSERT INTO nodes (id, project_id, seq, parent_node_id, branch_id, kind, created_ms,
                input_version_id, rule_id, payload_json)
             VALUES (?1,?2,1,NULL,?3,'import',?4,?5,?6,?7)",
            rusqlite::params![
                import_node, project_id, MAIN_BRANCH, ts, iv, rule,
                serde_json::json!({
                    "station": payload.station,
                    "epoch_count": payload.epochs.len(),
                    "diagnostic_count": diags.len(),
                    "raw_fingerprint": crate::id::content_fp(raw_text),
                }).to_string()
            ],
        ).map_err(|e| e.to_string())?;

        let accepted: BTreeSet<(String, String)> = BTreeSet::new();
        let output = crate::analysis::analyze(project_id, &iv, &rule, &payload.epochs, &params, &accepted);
        let ts2 = now(&self.store);
        tx.execute(
            "INSERT INTO nodes (id, project_id, seq, parent_node_id, branch_id, kind, created_ms,
                input_version_id, rule_id, payload_json)
             VALUES (?1,?2,2,?3,?4,'analysis',?5,?6,?7,?8)",
            rusqlite::params![
                analysis_node, project_id, import_node, MAIN_BRANCH, ts2, iv, rule,
                serde_json::json!({
                    "candidate_count": output.candidates.len(),
                    "clock_count": output.clocks.len(),
                    "arc_count": output.arcs.len(),
                }).to_string()
            ],
        ).map_err(|e| e.to_string())?;
        store_result(&tx, project_id, &analysis_node, MAIN_BRANCH, &iv, &rule, &accepted, &output)
            .map_err(|e| e.to_string())?;
        drop(tx);
        self.state_at(project_id, MAIN_BRANCH)
    }

    fn head(&self, project_id: &str, branch: &str) -> rusqlite::Result<NodeView> {
        let c = self.store.conn.lock().unwrap();
        head_node(&c, project_id, branch)
    }

    /// Record a parameter revision and recompute. Only results that depend on
    /// changed inputs become stale; identical fingerprints reuse the rule.
    pub fn revise_params(&self, project_id: &str, params: Params, note: &str) -> Result<StateView, String> {
        let head = self.head(project_id, MAIN_BRANCH).map_err(|e| e.to_string())?;
        let c = self.store.conn.lock().unwrap();
        let fp = params.fingerprint();
        let rule_id = crate::id::id("rule", &[project_id, &fp]);
        let exists: bool = c
            .query_row("SELECT 1 FROM rules WHERE id=?1", rusqlite::params![rule_id], |_| Ok(()))
            .is_ok();
        if !exists {
            let rseq = next_seq(&c, project_id, "rules", "seq").map_err(|e| e.to_string())?;
            upsert_rule(&c, project_id, &rule_id, rseq, &params, note).map_err(|e| e.to_string())?;
        }
        let input_id = head.input_version_id.unwrap();
        let epochs = load_epochs(&c, project_id, &input_id).map_err(|e| e.to_string())?;
        let accepted = accepted_edges(&c, project_id, MAIN_BRANCH).map_err(|e| e.to_string())?;
        let output = crate::analysis::analyze(project_id, &input_id, &rule_id, &epochs, &params, &accepted);
        let seq = next_node_seq(&c, project_id).map_err(|e| e.to_string())?;
        let pnode = crate::id::id("node", &[project_id, MAIN_BRANCH, &seq.to_string(), "param-revision"]);
        let ts = now(&self.store);
        c.execute(
            "INSERT INTO nodes (id, project_id, seq, parent_node_id, branch_id, kind, created_ms,
                input_version_id, rule_id, payload_json)
             VALUES (?1,?2,?3,?4,?5,'param_revision',?6,?7,?8,?9)",
            rusqlite::params![
                pnode, project_id, seq, head.id, MAIN_BRANCH, ts, input_id, rule_id,
                serde_json::json!({
                    "fingerprint": fp,
                    "algorithm_version": crate::params::ALGORITHM_VERSION,
                    "note": note,
                }).to_string()
            ],
        ).map_err(|e| e.to_string())?;
        let an = push_analysis(&c, self.store.clock.as_ref(), project_id, &pnode, MAIN_BRANCH,
            &input_id, &rule_id, &epochs, &params, &accepted, &output,
            "parameters revised").map_err(|e| e.to_string())?;
        let _ = an;
        drop(c);
        self.state_at(project_id, MAIN_BRANCH)
    }

    /// Apply a *local* source change: a new input version with a parent link.
    /// Only dependent results (the changed time keys) go stale; untouched
    /// satellite results carry forward and are still identity-stable.
    pub fn change_source(&self, project_id: &str, raw_text: &str, note: &str) -> Result<StateView, String> {
        let (payload, diags) = parse_import(raw_text)?;
        let head = self.head(project_id, MAIN_BRANCH).map_err(|e| e.to_string())?;
        let parent_iv = head.input_version_id.unwrap();
        let c = self.store.conn.lock().unwrap();
        let prev_times: BTreeSet<String> =
            load_epoch_times(&c, project_id, &parent_iv).map_err(|e| e.to_string())?.into_iter().collect();
        let new_times: BTreeSet<String> = payload.epochs.iter().map(|e| e.time.clone()).collect();
        let removed: Vec<String> = prev_times.difference(&new_times).cloned().collect();

        let seq = next_seq(&c, project_id, "input_versions", "seq").map_err(|e| e.to_string())?;
        let iv = crate::id::id("iv", &[project_id, &format!("v{seq}")]);
        write_input_snapshot(&c, project_id, &iv, seq, &payload.station, raw_text, &payload.epochs, &diags)
            .map_err(|e| e.to_string())?;
        link_input_parent(&c, &iv, &parent_iv).map_err(|e| e.to_string())?;
        record_removed_epochs(&c, &iv, &removed).map_err(|e| e.to_string())?;

        let changed_times: Vec<String> = {
            let old_map = load_epoch_map(&c, project_id, &parent_iv).map_err(|e| e.to_string())?;
            payload
                .epochs
                .iter()
                .filter(|e| old_map.get(&e.time).map(|old| old != e).unwrap_or(true))
                .map(|e| e.time.clone())
                .chain(removed.iter().cloned())
                .collect()
        };

        let rule_id = head.rule_id.unwrap();
        let params = load_params(&c, &rule_id).map_err(|e| e.to_string())?;
        let accepted = accepted_edges(&c, project_id, MAIN_BRANCH).map_err(|e| e.to_string())?;
        let output = crate::analysis::analyze(project_id, &iv, &rule_id, &payload.epochs, &params, &accepted);
        let nseq = next_node_seq(&c, project_id).map_err(|e| e.to_string())?;
        let pnode = crate::id::id("node", &[project_id, MAIN_BRANCH, &nseq.to_string(), "source-change"]);
        let ts = now(&self.store);
        c.execute(
            "INSERT INTO nodes (id, project_id, seq, parent_node_id, branch_id, kind, created_ms,
                input_version_id, rule_id, payload_json)
             VALUES (?1,?2,?3,?4,?5,'source_change',?6,?7,?8,?9)",
            rusqlite::params![
                pnode, project_id, nseq, head.id, MAIN_BRANCH, ts, iv, rule_id,
                serde_json::json!({
                    "parent_input_version_id": parent_iv,
                    "changed_times": changed_times,
                    "removed_times": removed,
                    "note": note,
                    "raw_fingerprint": crate::id::content_fp(raw_text),
                }).to_string()
            ],
        ).map_err(|e| e.to_string())?;
        push_analysis(&c, self.store.clock.as_ref(), project_id, &pnode, MAIN_BRANCH,
            &iv, &rule_id, &payload.epochs, &params, &accepted, &output,
            "local source change; dependent results invalidated")
            .map_err(|e| e.to_string())?;
        drop(c);
        self.state_at(project_id, MAIN_BRANCH)
    }
