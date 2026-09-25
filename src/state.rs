//! 复核视图：在某事件序号上折叠历史，并把"直接来源已变"的派生对象标记为过期。
//! 过期结果仍可查看（保留旧计算与旧来源/参数），但不能冒充当前结果。

use crate::decision::{Decision, DecisionAction, Event, EventKind};
use crate::derived::{DetectorRun, GroupStatus};
use crate::engine::run_detector;
use crate::model::RawImport;
use crate::params::DetectorParams;
use crate::project::{ParamsRevision, Project};

/// 一个派生对象的过期标记。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StaleNote {
    pub object_id: String,
    pub stale: bool,
    pub reason: Option<String>,
}

/// 历史回放结果：该时刻的来源、参数、决定与检测产物。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ReviewState {
    pub head_seq: u64,
    pub current: bool,
    pub source_import_id: String,
    pub param_fingerprint: String,
    pub run: DetectorRun,
    /// 组状态在叠加决定后的结果（含被取代决定的追溯信息）
    pub effective_decisions: Vec<Decision>,
    pub stale_notes: Vec<StaleNote>,
}

/// 从事件日志折叠出项目当前聚合。
pub fn fold_events(project_id: &str, events: &[Event]) -> Project {
    let mut project = Project {
        id: project_id.to_string(),
        name: String::new(),
        created_at: String::new(),
        imports: Vec::new(),
        params_revisions: Vec::new(),
        decisions: Vec::new(),
    };
    for ev in events {
        match ev.kind {
            EventKind::ProjectCreated => {
                project.name = ev.payload["name"].as_str().unwrap_or("").to_string();
                project.created_at = ev.at.clone();
            }
            EventKind::Imported => {
                let imp: RawImport = serde_json::from_value(ev.payload["import"].clone()).unwrap();
                project.imports.push(imp);
            }
            EventKind::ParamsRevised => {
                let rev: ParamsRevision = serde_json::from_value(ev.payload.clone()).unwrap();
                project.params_revisions.push(rev);
            }
            EventKind::DecisionRecorded => {
                let d: Decision = serde_json::from_value(ev.payload["decision"].clone()).unwrap();
                project.decisions.push(d);
            }
            EventKind::ForkCreated => {}
        }
    }
    project
}

/// 计算当前（最新）检测运行。
pub fn current_run(project: &Project) -> DetectorRun {
    run_detector(project.current_import(), project.current_params(), &project.accepted_offsets())
}

/// 在给定事件序号上回放（`at_seq` 包含）。返回当时的检测结果。
pub fn state_at_seq(
    project_id: &str,
    all_events: &[Event],
    at_seq: u64,
    current: &DetectorRun,
) -> Option<ReviewState> {
    let slice: Vec<Event> = all_events.iter().filter(|e| e.seq <= at_seq).cloned().collect();
    if slice.is_empty() {
        return None;
    }
    let mut project = fold_events(project_id, &slice);
    if project.imports.is_empty() || project.params_revisions.is_empty() {
        return None;
    }
    // 被取代的决定在回放时刻仍然有效（历史不可被最新结果覆盖）。
    let run = run_detector(project.current_import(), project.current_params(), &project.accepted_offsets());

    // 与当前对象集合比较：身份不存在 => 该结果的直接来源/参数已变化 => 过期。
    let cur_trn: std::collections::HashSet<String> = current.transitions.iter().map(|t| t.id.clone()).collect();
    let cur_cmb: std::collections::HashSet<String> = current.combos.iter().map(|c| c.id.clone()).collect();
    let cur_grp: std::collections::HashSet<String> = current.groups.iter().map(|g| g.id.clone()).collect();
    let cur_arc: std::collections::HashSet<String> = current.arcs.iter().map(|a| a.id.clone()).collect();

    let mut notes = Vec::new();
    for t in &run.transitions {
        let stale = !cur_trn.contains(&t.id);
        notes.push(StaleNote { object_id: t.id.clone(), stale, reason: stale.then(|| "source epoch or rule version changed".into()) });
    }
    for c in &run.combos {
        let stale = !cur_cmb.contains(&c.id);
        notes.push(StaleNote { object_id: c.id.clone(), stale, reason: stale.then(|| "source epoch or rule version changed".into()) });
    }
    for g in &run.groups {
        let stale = !cur_grp.contains(&g.id);
        notes.push(StaleNote { object_id: g.id.clone(), stale, reason: stale.then(|| "source epoch or rule version changed".into()) });
    }
    for a in &run.arcs {
        let stale = !cur_arc.contains(&a.id);
        notes.push(StaleNote { object_id: a.id.clone(), stale, reason: stale.then(|| "arc boundaries recomputed from newer source".into()) });
    }

    Some(ReviewState {
        head_seq: at_seq,
        current: false,
        source_import_id: project.current_import().id.clone(),
        param_fingerprint: project.current_params().fingerprint(),
        run,
        effective_decisions: project.decisions,
        stale_notes: notes,
    })
}

/// 当前视图（无过期）。
pub fn current_state(project: &Project) -> ReviewState {
    let run = current_run(project);
    ReviewState {
        head_seq: project.decisions.len() as u64 + project.imports.len() as u64 + project.params_revisions.len() as u64 + 1,
        current: true,
        source_import_id: project.current_import().id.clone(),
        param_fingerprint: project.current_params().fingerprint(),
        run,
        effective_decisions: project.decisions.clone(),
        stale_notes: vec![],
    }
}
