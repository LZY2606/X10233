//! 项目聚合：导入版本链、参数修订、决定集合（事件溯源后折叠的当前状态）。

use crate::decision::Decision;
use crate::model::RawImport;
use crate::params::DetectorParams;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamsRevision {
    pub fingerprint: String,
    pub algo_version: String,
    pub params: DetectorParams,
    pub note: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub created_at: String,
    /// 导入版本（追加，不可覆盖），按 version 排序。
    pub imports: Vec<RawImport>,
    pub params_revisions: Vec<ParamsRevision>,
    pub decisions: Vec<Decision>,
}

impl Project {
    pub fn current_import(&self) -> &RawImport {
        self.imports.last().expect("project has at least one import")
    }
    pub fn current_params(&self) -> &DetectorParams {
        &self.params_revisions.last().expect("params initialized").params
    }
    /// 当前已采纳偏移：(epoch, sat) -> {band: cycles}；否定决定存空 map。
    pub fn accepted_offsets(
        &self,
    ) -> std::collections::BTreeMap<(usize, String), std::collections::BTreeMap<u8, i64>> {
        let mut out = std::collections::BTreeMap::new();
        for d in &self.decisions {
            let key = (d.epoch_index, d.satellite.clone());
            use crate::decision::DecisionAction;
            match d.action {
                DecisionAction::Accept => {
                    out.insert(key, d.offsets.clone());
                }
                DecisionAction::Reject => {
                    out.insert(key, std::collections::BTreeMap::new());
                }
            }
        }
        out
    }
}
