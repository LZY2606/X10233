//! Bundled offline case pack: "载波断点台" (carrier breakpoint bench).
//!
//! Both CBF texts and the manifest are embedded in the binary, so the demo
//! and tests work fully offline from `cargo run` / `cargo test`.

use serde::{Deserialize, Serialize};

pub const BASELINE_CBF: &str = include_str!("../cases/carrier-breakpoint/v1-baseline.cbf");
pub const TAIL_CBF: &str = include_str!("../cases/carrier-breakpoint/v2-tail-unequal.cbf");
pub const MANIFEST_JSON: &str = include_str!("../cases/carrier-breakpoint/manifest.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseManifest {
    pub id: String,
    pub title: String,
    pub format: String,
    pub station: String,
    pub seed: i64,
    pub time_datum: String,
    pub coordinate_datum: String,
    pub units: std::collections::BTreeMap<String, String>,
    pub steps: Vec<CaseStep>,
    pub detector_contract: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseStep {
    pub order: i64,
    pub kind: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub asserts: Vec<String>,
    #[serde(default)]
    pub fork_from: Option<String>,
}

pub fn manifest() -> CaseManifest {
    serde_json::from_str(MANIFEST_JSON).expect("embedded manifest is valid JSON")
}

pub fn case_file(name: &str) -> Option<&'static str> {
    match name {
        "v1-baseline.cbf" => Some(BASELINE_CBF),
        "v2-tail-unequal.cbf" => Some(TAIL_CBF),
        _ => None,
    }
}
