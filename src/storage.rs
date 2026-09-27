use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde_json::Value;

use crate::engine::{compute, ComputeInput};
use crate::model::{
    normalize_value, ArcSegment, CandidateGroup, CasePackage, ClockEvent, DetectorParameters,
    EpochDef, FrequencyDef, NormalizedValue, RawObservation, RawObservationInput, RepairOffset,
    RevisionPackage, SatelliteDef, SatelliteState, Station, ALGORITHM_VERSION,
};
use crate::validation::{validate_case, validate_revision};

pub struct Database {
    pub connection: Connection,
}

#[derive(Debug, Clone)]
pub struct DecisionRequest {
    pub branch_id: String,
    pub parent_event_id: String,
    pub candidate_group_id: String,
    pub action: String,
    pub selected_offset_cycles: Option<f64>,
    pub reason: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectView {
    pub project_id: String,
    pub case_id: String,
    pub title: String,
    pub narrative: String,
    pub branches: Vec<Value>,
    pub events: Vec<Value>,
    pub station: Value,
    pub epochs: Vec<Value>,
    pub satellites: Vec<Value>,
    pub frequencies: Vec<Value>,
    pub satellite_states: Vec<Value>,
    pub clock_events: Vec<Value>,
    pub raw_observations: Vec<Value>,
    pub normalized_values: Vec<Value>,
    pub residuals: Vec<Value>,
    pub candidate_groups: Vec<Value>,
    pub repair_offsets: Vec<Value>,
    pub arcs: Vec<Value>,
}

impl Database {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut connection = Connection::open(path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut connection)?;
        Ok(Self { connection })
    }

    pub fn import_case(&mut self, package: CasePackage) -> Result<String, String> {
        validate_case(&package).map_err(|errors| errors.join("; "))?;
        if self.project_exists(&package.case_id)? {
            return Err("project already exists".into());
        }
        let tx = self.connection.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "insert into projects(project_id, case_id, title, narrative, created_virtual_time) values (?1,?2,?3,?4,'2026-09-27T00:00:00Z')",
            params![package.case_id, package.case_id, package.title, package.narrative],
        ).map_err(|e| e.to_string())?;
        tx.execute(
            "insert into branches(branch_id, project_id, name, parent_branch_id, head_event_id, created_virtual_time) values (?1,?2,'main',null,?3,'2026-09-27T00:00:00Z')",
            params![package.case_id, package.case_id, "import-0001"],
        ).map_err(|e| e.to_string())?;
        let event_payload = serde_json::json!({
            "kind": "import",
            "case_id": package.case_id,
            "immutable_raw_policy": "submitted raw observations are never overwritten; revisions create child versions",
            "judgment_basis": [
                {"id": "basis-detectors-independent", "type": "detector_policy", "statement": "ionosphere, Melbourne-Wubbena and time-difference are independent candidates, not truth"},
                {"id": "basis-clock-separation", "type": "classification_policy", "statement": "whole-receiver clock jumps are separated from single-satellite slips"},
                {"id": "basis-offset-layer", "type": "repair_policy", "statement": "accepted repairs are stored only as offset layers"}
            ]
        });
        insert_event(&tx, &package.case_id, "main", "import-0001", None, "import", event_payload, "2026-09-27T00:00:00Z")?;
        insert_entities(&tx, &package)?;
        let raw = insert_raw_observations(&tx, &package.case_id, "import-0001", &package.observations)?;
        insert_normalized(&tx, &package.case_id, "import-0001", &raw)?;
        let state = build_state_at(&tx, &package.case_id, "import-0001")?;
        insert_computed_state(&tx, &package.case_id, "import-0001", state)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(package.case_id)
    }

    fn project_exists(&self, project_id: &str) -> rusqlite::Result<bool> {
        let mut stmt = self.connection.prepare("select 1 from projects where project_id=?1")?;
        Ok(stmt.exists(params![project_id])?)
    }
