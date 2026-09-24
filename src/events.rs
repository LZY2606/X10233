//! 事件溯源：所有写操作都是不可变事件；修订以新版本 + 父版本关系表达。
use serde::{Deserialize, Serialize};

use crate::identity::digest16;
use crate::model::{CommitIn, DetectorParams};

pub const ALGO_VERSION: &str = "cycle-slip-core-1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    /// 分支头链：事件按 (project, parent) 构成树。
    pub kind: String,
    pub created_at: String,
    pub payload: serde_json::Value,
    /// 派生结果算法版本。
    pub algo_version: String,
    /// 本事件生效的参数指纹。
    pub params_fp: String,
    /// 对某事件的撤销/冲突说明（kind=decision_revert/conflict 时使用）。
    #[serde(default)]
    pub note: String,
}

pub fn params_fingerprint(p: &DetectorParams) -> String {
    digest16(&p)
}

/// project_created
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectCreated {
    pub project_id: String,
    pub title: String,
}

/// raw_committed：原始输入一经提交不可覆盖。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawCommitted {
    pub commit_label: String,
    pub commit: CommitIn,
    /// 解析诊断（格式错误仍记录，但不阻止良好片段导入）。
    #[serde(default)]
    pub diagnostics: Vec<crate::model::Diagnostic>,
}

/// raw_amended：源数据修订，以新版本与父版本关系表达（不覆盖旧数据）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawAmended {
    /// 仅这些 (epoch_time, sat, code) 的观测被新版本替换；其余引用旧版。
    pub target: AmendmentTarget,
    pub replacement: crate::model::ObservationIn,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmendmentTarget {
    pub time: String,
    pub sat: String,
    pub code: String,
}

/// params_revised
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamsRevised {
    pub old_fp: String,
    pub new_fp: String,
    pub params: DetectorParams,
    pub reason: String,
}

/// 人工裁定：accepted / rejected / overridden（分叉重算的基础）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionMade {
    pub candidate_id: String,
    /// accepted | rejected | overridden
    pub verdict: String,
    /// 接受时写入偏移层的整周偏移（覆盖/指定），可空。
    #[serde(default)]
    pub chosen: Option<ChosenOffset>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChosenOffset {
    pub n1: i32,
    pub n2: i32,
}

/// 分叉：从某个旧事件另起一条链。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchForked {
    pub fork_from_event: String,
    pub branch_name: String,
}

/// 撤销：产生新事件记录撤销动作（不删除被撤销事件）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionReverted {
    pub reverted_event: String,
    pub candidate_id: String,
    pub reason: String,
}
