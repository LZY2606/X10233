//! 应用服务：计算、局部过期、历史视图、分叉重算、导出/导入。

use super::engine::{self, Candidate, CandidateState, Detector, RunResult};
use super::events::{Event, EventPayload};
use crate::infra::store::{self, DecisionRec, Store};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateView {
    #[serde(flatten)]
    pub candidate: Candidate,
    /// 人工选定的偏移（仅偏移层），未选为 None。
    pub applied_offset_b1: Option<i64>,
    pub applied_offset_b2: Option<i64>,
    pub decision_seq: Option<i64>,
    pub decision_note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunView {
    pub run_id: String,
    pub algorithm_version: String,
    pub param_fingerprint: String,
    pub fresh: bool,
    pub stale_reasons: Vec<String>,
    pub candidates: Vec<CandidateView>,
    pub clock_jump_count: usize,
    pub gf_series: BTreeMap<String, Vec<engine::ResidualPoint>>,
    pub mw_series: BTreeMap<String, Vec<engine::ResidualPoint>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Projection {
    pub project_id: String,
    pub project_name: String,
    pub station: String,
    pub branch_id: String,
    pub head_seq: i64,
    pub params_version: u64,
    pub params: engine::DetectorParams,
    pub commit_count: usize,
    pub clock_event_count: usize,
    pub sat_state_count: usize,
    pub arcs: Vec<engine::Arc>,
    pub run: Option<RunView>,
    /// 所有历史 run（含过期），供时间线查看。
    pub runs_index: Vec<RunIndexEntry>,
    pub imported_clock_events: Vec<super::model::ClockEventInput>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunIndexEntry {
    pub run_id: String,
    pub fresh: bool,
    pub computed_at_seq: i64,
}

#[derive(Clone, Debug)]
struct RunCtx {
    at_seq: i64,
    run: RunResult,
    params_fp: String,
    slots: BTreeSet<String>,
    decisions: Vec<DecisionRec>,
}

/// 基于事件重建一个分支在某个 seq（含）的可见状态。
pub fn project_at(
    project_id: &str,
    branch_id: &str,
    events: &[Event],
    station: &str,
    project_name: &str,
) -> Projection {
    let mut state = store::replay(project_name, station, events);
    let head_seq = events.last().map(|e| e.seq).unwrap_or(0);

    // 逐次重建所有 computed run 的上下文（历史 run 不被覆盖，只标记过期）。
    let mut ctxs: Vec<RunCtx> = Vec::new();
    let mut commits_now: Vec<(String, engine::ParsedImport)> = Vec::new();
    let mut clocks_now: Vec<super::model::ClockEventInput> = Vec::new();
    let mut params_now = engine::DetectorParams::default();
    let mut decisions_all: Vec<DecisionRec> = Vec::new();
    for e in events {
        match &e.payload {
            EventPayload::ObservationsCommitted { commit_id, parsed, .. } => {
                commits_now.push((commit_id.clone(), parsed.clone()));
            }
            EventPayload::ClockEventsCommitted { events: evs } => clocks_now.extend(evs.iter().cloned()),
            EventPayload::ParamsRevised { params, .. } => params_now = params.clone(),
            EventPayload::Computed { run_id, .. } => {
                let run = engine::compute_run(run_id, station, &commits_now, &clocks_now, &params_now);
                let slots: BTreeSet<String> = run
                    .candidates
                    .iter()
                    .flat_map(|c| c.source_slot_ids.iter().cloned())
                    .chain(run.arcs.iter().flat_map(|a| a.slot_ids.iter().cloned()))
                    .collect();
                let decs: Vec<DecisionRec> = decisions_all.iter().cloned().collect();
                ctxs.push(RunCtx {
                    at_seq: e.seq,
                    run,
                    params_fp: params_now.fingerprint(),
                    slots,
                    decisions: decs,
                });
            }
            EventPayload::DecisionMade { .. } => {
                state.decisions.clear();
                let mut partial = Vec::new();
                for ev in events.iter().filter(|ev| ev.seq <= e.seq) {
                    if let EventPayload::DecisionMade { run_id, candidate_id, action, chosen_offset_b1, chosen_offset_b2, note, .. } = &ev.payload {
                        partial.push(DecisionRec {
                            run_id: run_id.clone(),
                            candidate_id: candidate_id.clone(),
                            action: action.clone(),
                            b1: *chosen_offset_b1,
                            b2: *chosen_offset_b2,
                            note: note.clone(),
                            at_seq: ev.seq,
                        });
                    }
                }
                decisions_all = partial;
            }
            _ => {}
        }
    }

    // 当前源数据：槽位集合 + 参数指纹（判断过期）。
    let current = engine::build_dataset(station, &commits_now, &clocks_now);
    let mut current_slots: BTreeSet<String> = BTreeSet::new();
    for s in current.series.values() {
        current_slots.extend(s.slot_ids.iter().cloned());
    }
    let current_fp = params_now.fingerprint();

    // 最新弧段（弧段边界按当前数据展示）。
    let arcs = engine::build_arcs(&current, &params_now);

    let mut runs_index = Vec::new();
    let mut run_view: Option<RunView> = None;
    if let Some(ctx) = ctxs.last() {
        let mut fresh = true;
        let mut stale_reasons = Vec::new();
        if ctx.params_fp != current_fp {
            fresh = false;
            stale_reasons.push("params_revised".into());
        }
        // 局部：只依赖变化槽位的候选过期；槽位不存在（源被修订替换）则相关候选过期。
        let changed_or_gone: BTreeSet<String> =
            ctx.slots.symmetric_difference(&current_slots).cloned().collect();
        let mut run = ctx.run.clone();
        let mut any_source_stale = false;
        for c in run.candidates.iter_mut() {
            let touched = c.source_slot_ids.iter().any(|id| changed_or_gone.contains(id));
            if touched {
                c.stale = true;
                c.stale_reasons.push("source_slot_changed".into());
                any_source_stale = true;
            }
            if c.param_fingerprint != current_fp && params_affect(c.detector) {
                c.stale = true;
                c.stale_reasons.push("params_revised".into());
            }
        }
        if any_source_stale {
            fresh = false;
            stale_reasons.push("source_data_changed_local".into());
        }
        let decs = decisions_all.iter().filter(|d| d.run_id == ctx.run.run_id).cloned().collect::<Vec<_>>();
        store::apply_decisions(&mut run, &decs);
        let candidates = run
            .candidates
            .into_iter()
            .map(|c| {
                let d = decs.iter().filter(|d| d.candidate_id == c.id).last();
                CandidateView {
                    applied_offset_b1: d.and_then(|d| d.b1),
                    applied_offset_b2: d.and_then(|d| d.b2),
                    decision_seq: d.map(|d| d.at_seq),
                    decision_note: d.map(|d| d.note.clone()).unwrap_or_default(),
                    candidate: c,
                }
            })
            .collect();
        run_view = Some(RunView {
            run_id: ctx.run.run_id.clone(),
            algorithm_version: ctx.run.algorithm_version.clone(),
            param_fingerprint: ctx.run.param_fingerprint.clone(),
            fresh,
            stale_reasons,
            candidates,
            clock_jump_count: ctx.run.clock_jumps.len(),
            gf_series: ctx.run.gf_series.clone(),
            mw_series: ctx.run.mw_series.clone(),
        });
        for cx in &ctxs {
            let f = cx.at_seq == ctx.at_seq;
            runs_index.push(RunIndexEntry { run_id: cx.run.run_id.clone(), fresh: f, computed_at_seq: cx.at_seq });
        }
    }

    Projection {
        project_id: project_id.to_string(),
        project_name: project_name.to_string(),
        station: station.to_string(),
        branch_id: branch_id.to_string(),
        head_seq,
        params_version: state.params_version,
        params: params_now,
        commit_count: commits_now.len(),
        clock_event_count: clocks_now.len(),
        sat_state_count: state.sat_states.len(),
        arcs,
        run: run_view,
        runs_index,
        imported_clock_events: clocks_now,
    }
}

fn params_affect(det: Detector) -> bool {
    // LLI 审阅位不依赖数值参数，参数修订不使其过期。
    !matches!(det, Detector::Lli)
}

/// 导出：项目 + 全部分支事件。身份为内容哈希，跨库导入保持稳定。
#[derive(Serialize, Deserialize)]
pub struct ProjectBundle {
    pub format: String,
    pub schema_version: u32,
    pub project_id: String,
    pub name: String,
    pub station: String,
    pub branches: Vec<BranchBundle>,
}

#[derive(Serialize, Deserialize)]
pub struct BranchBundle {
    pub branch_id: String,
    pub name: String,
    pub events: Vec<Event>,
}

pub fn export_bundle(store: &Store, project_id: &str) -> rusqlite::Result<ProjectBundle> {
    let (name, station) = store.project_station(project_id)?.unwrap_or_default();
    let branches = store.list_branches(project_id)?;
    let mut out = Vec::new();
    for b in branches {
        out.push(BranchBundle {
            branch_id: b.clone(),
            name: b,
            events: store.events(project_id, &b)?,
        });
    }
    Ok(ProjectBundle {
        format: "cycleslip-arc-bundle".into(),
        schema_version: 1,
        project_id: project_id.to_string(),
        name,
        station,
        branches: out,
    })
}

pub fn import_bundle(store: &Store, bundle: &ProjectBundle, clock: &dyn Clock) -> rusqlite::Result<String> {
    store.create_project(&bundle.project_id, &bundle.name, &bundle.station, clock)?;
    for br in &bundle.branches {
        if br.branch_id == "main" {
            continue;
        }
        let exists = store.list_branches(&bundle.project_id)?.iter().any(|b| b == &br.branch_id);
        if !exists {
            store.create_branch(&bundle.project_id, &br.branch_id, &br.name, clock)?;
            for e in &br.events {
                let conn = store.conn_lock();
                conn.execute(
                    "INSERT OR IGNORE INTO events(project_id,branch_id,seq,id,created_at,payload_json) VALUES(?1,?2,?3,?4,?5,?6)",
                    rusqlite::params![
                        bundle.project_id,
                        br.branch_id,
                        e.seq,
                        e.id,
                        e.created_at,
                        serde_json::to_string(&e.payload).unwrap()
                    ],
                )?;
            }
        }
    }
    // main：以原 seq/id 原样导入，保证身份、顺序、来源关系不变。
    if let Some(main) = bundle.branches.iter().find(|b| b.branch_id == "main") {
        let conn = store.conn_lock();
        for e in &main.events {
            conn.execute(
                "INSERT OR IGNORE INTO events(project_id,branch_id,seq,id,created_at,payload_json) VALUES(?1,'main',?2,?3,?4,?5)",
                rusqlite::params![bundle.project_id, e.seq, e.id, e.created_at, serde_json::to_string(&e.payload).unwrap()],
            )?;
        }
    }
    Ok(bundle.project_id.clone())
}
