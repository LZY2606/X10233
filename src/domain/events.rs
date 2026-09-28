//! 事件溯源模型：原始输入一经提交不可覆盖；修订以新版本 + 父版本表达；人工裁定、分叉、参数修订都是事件。

use super::engine::DetectorParams;
use super::model::{ClockEventInput, ParsedImport, SatelliteStateInput};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventPayload {
    ProjectCreated {
        name: String,
        station: String,
    },
    ObservationsCommitted {
        commit_id: String,
        parent_commit_id: Option<String>,
        source_name: String,
        parsed: ParsedImport,
    },
    ClockEventsCommitted {
        events: Vec<ClockEventInput>,
    },
    SatStatesCommitted {
        states: Vec<SatelliteStateInput>,
    },
    ParamsRevised {
        version: u64,
        parent_version: u64,
        params: DetectorParams,
        reason: String,
    },
    Computed {
        run_id: String,
        parent_run_id: Option<String>,
        algorithm_version: String,
        param_fingerprint: String,
    },
    DecisionMade {
        run_id: String,
        candidate_id: String,
        /// accept / reject / hypothesis_selected / reset
        action: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        chosen_offset_b1: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        chosen_offset_b2: Option<i64>,
        note: String,
        /// 与该候选既有决定冲突时，仍写入事件并记录被取代的决定。
        conflicts_with: Vec<String>,
    },
    BranchCreated {
        branch_id: String,
        name: String,
        from_event_seq: i64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Event {
    /// 分支内递增序号。
    pub seq: i64,
    pub id: String,
    pub branch_id: String,
    /// 全局时间戳：真实运行用墙钟，测试用虚拟时间。
    pub created_at: String,
    pub payload: EventPayload,
}
