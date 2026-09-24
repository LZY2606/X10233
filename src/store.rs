use crate::engine::{self, Clock, Frequency, ObsField, Repair};
use crate::hash::{source_hash, stable_id};
use crate::model::{
    parse_observations, parse_status, DecisionRequest, RawField, RuleParams, ALGORITHM_VERSION,
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
    clock_ns: Mutex<u128>,
}

#[derive(Debug)]
pub struct ImportError(pub String);

#[derive(Clone)]
struct Active {
    input_version_id: String,
    observation_text: String,
    status_text: String,
    params: RuleParams,
    repairs: Vec<Repair>,
}

impl Store {
    pub fn open(path: &str) -> Result<Self, String> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()
        } else {
            Connection::open(path)
        }
        .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "foreign_keys", "on").ok();
        conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(Self { conn: Mutex::new(conn), clock_ns: Mutex::new(8_000_000_000) })
    }

    fn now(&self) -> String {
        let mut value = self.clock_ns.lock().unwrap();
        *value += 1;
        let whole = *value / 1_000_000_000;
        let fraction = *value % 1_000_000_000;
        format!("2026-09-25T09:{whole:02}:{fraction:09}Z")
    }

    pub fn create_project(&self, name: &str, observation_text: &str, status_text: &str) -> Result<Value, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let project_id = stable_id(&["project", name]);
        tx.execute("insert into projects(id,name,created_at,updated_at) values(?1,?2,?3,?3)", params![project_id, name, self.now()]).map_err(|e| e.to_string())?;
        let created = self.now();
        tx.execute("insert into events(id,project_id,seq,kind,created_at,payload) values(?1,?2,1,'project-created',?3,?4)",
            params![stable_id(&[&project_id, "event", "1"]), project_id, created, json!({"name": name}).to_string()]).map_err(|e| e.to_string())?;
        let input_version_id = stable_id(&[&project_id, "input", "1"]);
        tx.execute("insert into input_versions(id,project_id,version_no,parent_id,kind,source_hash,observation_sha,status_sha,created_event_id) values(?1,?2,1,null,'initial',?3,?4,?5,?6)",
            params![input_version_id, project_id, source_hash(&format!("{observation_text}\0{status_text}")), source_hash(observation_text), source_hash(status_text), stable_id(&[&project_id, "event", "1"])]).map_err(|e| e.to_string())?;
        insert_sources_and_parse(&tx, &project_id, &input_version_id, observation_text, status_text)?;
        let params = RuleParams::default();
        let run_id = stable_id(&[&project_id, "run", "1"]);
        compute_and_insert_run(&tx, &project_id, &input_version_id, &run_id, 1, observation_text, status_text, &params, &[], None)?;
        tx.execute("update projects set updated_at=?1 where id=?2", params![self.now(), project_id]).ok();
        tx.commit().map_err(|e| e.to_string())?;
        Ok(project_json(&self.conn.lock().unwrap(), &project_id)?)
    }

    pub fn revise_sources(&self, project_id: &str, kind: &str, observation_text: &str, status_text: &str, reason: &str) -> Result<Value, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let active = active(&tx, project_id)?;
        let next_seq = next_seq(&tx, project_id)?;
        let next_version_no = next_version_no(&tx, project_id)?;
        let event_id = stable_id(&[project_id, "event", &next_seq.to_string()]);
        let created = self.now();
        tx.execute("insert into events(id,project_id,seq,kind,created_at,payload) values(?1,?2,?3,?4,?5,?6)",
            params![event_id, project_id, next_seq, if kind == "source" { "source-revised" } else { "rules-revised" }, created, json!({"reason": reason}).to_string()]).map_err(|e| e.to_string())?;
        let observation_text = if observation_text.is_empty() { active.observation_text.clone() } else { observation_text.to_string() };
        let status_text = if status_text.is_empty() { active.status_text.clone() } else { status_text.to_string() };
        let input_version_id = stable_id(&[project_id, "input", &next_version_no.to_string()]);
        tx.execute("insert into input_versions(id,project_id,version_no,parent_id,kind,source_hash,observation_sha,status_sha,created_event_id) values(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![input_version_id, project_id, next_version_no, active.input_version_id, kind, source_hash(&format!("{observation_text}\0{status_text}")), source_hash(&observation_text), source_hash(&status_text), event_id]).map_err(|e| e.to_string())?;
        insert_sources_and_parse(&tx, project_id, &input_version_id, &observation_text, &status_text)?;
        let run_id = stable_id(&[project_id, "run", &next_seq.to_string()]);
        compute_and_insert_run(&tx, project_id, &input_version_id, &run_id, next_seq, &observation_text, &status_text, &active.params, &active.repairs, Some(event_id))?;
        mark_stale(&tx, project_id, &run_id)?;
        tx.execute("update projects set updated_at=?1 where id=?2", params![created, project_id]).ok();
        tx.commit().map_err(|e| e.to_string())?;
        Ok(project_json(&self.conn.lock().unwrap(), project_id)?)
    }

    pub fn revise_rules(&self, project_id: &str, params: RuleParams, reason: &str) -> Result<Value, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let active = active(&tx, project_id)?;
        let next_seq = next_seq(&tx, project_id)?;
        let event_id = stable_id(&[project_id, "event", &next_seq.to_string()]);
        let created = self.now();
        let fp = engine::parameter_fingerprint(&params);
        tx.execute("insert into events(id,project_id,seq,kind,created_at,payload) values(?1,?2,?3,'rules-revised',?4,?5)",
            params![event_id, project_id, next_seq, created, json!({"reason": reason, "params": params, "fingerprint": fp}).to_string()]).map_err(|e| e.to_string())?;
        let run_id = stable_id(&[project_id, "run", &next_seq.to_string()]);
        compute_and_insert_run(&tx, project_id, &active.input_version_id, &run_id, next_seq, &active.observation_text, &active.status_text, &params, &active.repairs, Some(event_id))?;
        mark_stale(&tx, project_id, &run_id)?;
        tx.execute("update projects set updated_at=?1 where id=?2", params![created, project_id]).ok();
        tx.commit().map_err(|e| e.to_string())?;
        Ok(project_json(&self.conn.lock().unwrap(), project_id)?)
    }

    pub fn decide(&self, project_id: &str, req: &DecisionRequest) -> Result<Value, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let active = active(&tx, project_id)?;
        let candidate: (String, String, String, Option<f64>, Option<f64>, String) = tx.query_row(
            "select sat,epoch,band,proposed_cycles_l1,proposed_cycles_l2,status from candidates where run_id=?1 and stable_identity=?2",
            params![req.parent_run_id, req.candidate_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        ).map_err(|_| "candidate not found in parent run".to_string())?;
        if req.action != "accept" && req.action != "reject" && req.action != "override" {
            return Err("action must be accept, reject, or override".into());
        }
        let next_seq = next_seq(&tx, project_id)?;
        let event_id = stable_id(&[project_id, "event", &next_seq.to_string()]);
        let created = self.now();
        let event_kind = if req.action == "accept" { "candidate-accepted" } else if req.action == "reject" { "candidate-rejected" } else { "decision-overridden" };
        tx.execute("insert into events(id,project_id,seq,kind,created_at,payload) values(?1,?2,?3,?4,?5,?6)",
            params![event_id, project_id, next_seq, event_kind, created, serde_json::to_value(req).unwrap().to_string()]).map_err(|e| e.to_string())?;
        let mut repairs = active.repairs;
        if req.action == "accept" {
            if candidate.3.is_none() {
                return Err("flag-only candidates cannot become repair offsets".into());
            }
            if candidate.2 == "L1/L2" {
                repairs.push(Repair { sat: candidate.0.clone(), epoch: candidate.1.clone(), band: "1".into(), cycles: candidate.3.unwrap() });
                repairs.push(Repair { sat: candidate.0.clone(), epoch: candidate.1.clone(), band: "2".into(), cycles: candidate.4.unwrap() });
            } else {
                repairs.push(Repair { sat: candidate.0.clone(), epoch: candidate.1.clone(), band: candidate.2.clone(), cycles: candidate.3.unwrap() });
            }
        }
        let params = req.integer_search_radius.map(|radius| RuleParams { integer_search_radius: radius, ..active.params.clone() }).unwrap_or(active.params.clone());
        let run_id = stable_id(&[project_id, "run", &next_seq.to_string()]);
        compute_and_insert_run(&tx, project_id, &active.input_version_id, &run_id, next_seq, &active.observation_text, &active.status_text, &params, &repairs, Some(event_id))?;
        tx.execute("insert into decisions(id,run_id,parent_run_id,candidate_stable_identity,action,reason,created_event_id) values(?1,?2,?3,?4,?5,?6,?7)",
            params![stable_id(&[project_id, "decision", &next_seq.to_string()]), run_id, req.parent_run_id, req.candidate_id, req.action, req.reason, event_id]).map_err(|e| e.to_string())?;
        for repair in &repairs {
            tx.execute("insert or ignore into repairs(run_id,sat,epoch,band,cycles,source_candidate_id) values(?1,?2,?3,?4,?5,?6)",
                params![run_id, repair.sat, repair.epoch, repair.band, repair.cycles, req.candidate_id]).ok();
        }
        mark_stale(&tx, project_id, &run_id)?;
        tx.execute("update projects set updated_at=?1 where id=?2", params![created, project_id]).ok();
        tx.commit().map_err(|e| e.to_string())?;
        Ok(project_json(&self.conn.lock().unwrap(), project_id)?)
    }

    pub fn project(&self, id: &str) -> Result<Value, String> {
        project_json(&self.conn.lock().unwrap(), id)
    }

    pub fn projects(&self) -> Result<Value, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("select id,name,created_at,updated_at from projects order by created_at,id").map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |r| Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"created_at":r.get::<_,String>(2)?,"updated_at":r.get::<_,String>(3)?}))).map_err(|e| e.to_string())?;
        let values: Result<Vec<_>, _> = rows.collect();
        Ok(json!({"projects": values.map_err(|e| e.to_string())?}))
    }

    pub fn export_project(&self, id: &str) -> Result<Value, String> {
        let conn = self.conn.lock().unwrap();
        export_json(&conn, id)
    }

    pub fn import_project(&self, bundle: Value) -> Result<Value, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        import_json(&tx, bundle)?;
        tx.commit().map_err(|e| e.to_string())?;
        let id = bundle["project"]["id"].as_str().ok_or("missing project id")?;
        project_json(&self.conn.lock().unwrap(), id)
    }
}

const SCHEMA: &str = r#"
create table if not exists projects(
  id text primary key,
  name text not null,
  created_at text not null,
  updated_at text not null
);
create table if not exists events(
  id text primary key,
  project_id text not null references projects(id),
  seq integer not null,
  kind text not null,
  created_at text not null,
  payload text not null,
  unique(project_id,seq)
);
create table if not exists input_versions(
  id text primary key,
  project_id text not null references projects(id),
  version_no integer not null,
  parent_id text references input_versions(id),
  kind text not null,
  source_hash text not null,
  observation_sha text not null,
  status_sha text not null,
  created_event_id text not null references events(id),
  unique(project_id,version_no)
);
create table if not exists source_files(
  id text primary key,
  project_id text not null,
  input_version_id text not null references input_versions(id),
  role text not null,
  original_text text not null,
  original_representation text not null,
  content_hash text not null
);
create table if not exists frequencies(
  id text primary key,
  project_id text not null,
  input_version_id text not null references input_versions(id),
  system text not null,
  carrier_code text not null,
  mhz real not null,
  unit text not null,
  raw_representation text not null
);
create table if not exists obs_fields(
  id text primary key,
  project_id text not null,
  input_version_id text not null references input_versions(id),
  epoch text not null,
  t_seconds real not null,
  sat text not null,
  code text not null,
  kind text not null,
  raw_value text,
  value real,
  unit text not null,
  lli integer,
  status text not null,
  epoch_raw_line text not null,
  field_raw_line text not null,
  line_no integer not null
);
create table if not exists status_fields(
  id text primary key,
  project_id text not null,
  input_version_id text not null references input_versions(id),
  epoch text not null,
  t_seconds real not null,
  sat text not null,
  datum text not null,
  key text not null,
  raw_value text not null,
  value real,
  unit text not null,
  raw_line text not null,
  line_no integer not null
);
create table if not exists clock_events(
  id text primary key,
  project_id text not null,
  input_version_id text not null references input_versions(id),
  epoch text not null,
  t_seconds real not null,
  kind text not null,
  delta_seconds real not null,
  unit text not null,
  raw_representation text not null,
  line_no integer not null
);
create table if not exists diagnostics(
  id text primary key,
  project_id text not null,
  input_version_id text references input_versions(id),
  line_no integer not null,
  severity text not null,
  code text not null,
  message text not null,
  raw_line text not null
);
create table if not exists rules(
  id text primary key,
  project_id text not null,
  algorithm_version text not null,
  parameter_fingerprint text not null,
  params_json text not null
);
create table if not exists runs(
  id text primary key,
  project_id text not null references projects(id),
  seq integer not null,
  event_id text references events(id),
  input_version_id text not null references input_versions(id),
  algorithm_version text not null,
  parameter_fingerprint text not null,
  params_json text not null,
  created_at text not null,
  unique(project_id,seq)
);
create table if not exists arcs(
  id text not null,
  run_id text not null references runs(id),
  project_id text not null,
  sat text not null,
  carrier_code text not null,
  start_epoch text not null,
  end_epoch text not null,
  open integer not null,
  start_reason text not null,
  end_reason text,
  algorithm_version text not null,
  parameter_fingerprint text not null,
  input_version_id text not null,
  stale integer not null default 0,
  primary key(run_id,id)
);
create table if not exists boundaries(
  id text not null,
  run_id text not null references runs(id),
  project_id text not null,
  sat text not null,
  carrier_code text not null,
  epoch text not null,
  reason text not null,
  strength text not null,
  direct_source_field_id text not null,
  algorithm_version text not null,
  parameter_fingerprint text not null,
  stale integer not null default 0,
  primary key(run_id,id)
);
create table if not exists findings(
  id text not null,
  run_id text not null references runs(id),
  project_id text not null,
  sat text not null,
  epoch text not null,
  band text not null,
  kind text not null,
  state text not null,
  value real not null,
  unit text not null,
  message text not null,
  algorithm_version text not null,
  parameter_fingerprint text not null,
  stale integer not null default 0,
  primary key(run_id,id)
);
create table if not exists candidates(
  stable_identity text not null,
  run_id text not null references runs(id),
  project_id text not null,
  group_identity text,
  sat text not null,
  epoch text not null,
  band text not null,
  detector text not null,
  hypothesis text not null,
  proposed_cycles_l1 real,
  proposed_cycles_l2 real,
  residual_value real,
  residual_unit text,
  score real not null,
  status text not null,
  tie_group text,
  rejection_reason text,
  evidence_json text not null,
  algorithm_version text not null,
  parameter_fingerprint text not null,
  direct_source_field_id text,
  stale integer not null default 0,
  primary key(run_id,stable_identity)
);
create table if not exists repairs(
  id text primary key,
  run_id text not null references runs(id),
  sat text not null,
  epoch text not null,
  band text not null,
  cycles real not null,
  source_candidate_id text not null,
  layer text not null default 'offset-only',
  unique(run_id,sat,epoch,band)
);
create table if not exists decisions(
  id text primary key,
  run_id text not null references runs(id),
  parent_run_id text not null references runs(id),
  candidate_stable_identity text not null,
  action text not null,
  reason text not,
  created_event_id text not null references events(id)
);
create index if not exists idx_obs_lookup on obs_fields(input_version_id,sat,epoch,code);
create index if not exists idx_runs_project on runs(project_id,seq);
create index if not exists idx_candidates_lookup on candidates(project_id,run_id,epoch,sat);
"#;

fn insert_sources_and_parse(
    tx: &rusqlite::Transaction,
    project_id: &str,
    input_version_id: &str,
    observation_text: &str,
    status_text: &str,
) -> Result<(), String> {
    let obs_id = stable_id(&[input_version_id, "file", "observations"]);
    tx.execute("insert into source_files(id,project_id,input_version_id,role,original_text,original_representation,content_hash) values(?1,?2,?3,'rinex-observation-subset',?4,'text/plain; rinex-observation-subset',?5)",
        params![obs_id, project_id, input_version_id, observation_text, source_hash(observation_text)]).map_err(|e| e.to_string())?;
    let status_id = stable_id(&[input_version_id, "file", "satellite-status"]);
    tx.execute("insert into source_files(id,project_id,input_version_id,role,original_text,original_representation,content_hash) values(?1,?2,?3,'satellite-status',?4,'text/plain; satellite-status',?5)",
        params![status_id, project_id, input_version_id, status_text, source_hash(status_text)]).map_err(|e| e.to_string())?;

    let observations = parse_observations(observation_text)?;
    let status = parse_status(status_text)?;
    for frequency in &observations.frequencies {
        tx.execute("insert into frequencies(id,project_id,input_version_id,system,carrier_code,mhz,unit,raw_representation) values(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![stable_id(&[input_version_id, "frequency", &frequency.system, &frequency.carrier_code]), project_id, input_version_id, frequency.system, frequency.carrier_code, frequency.mhz, frequency.unit, frequency.raw]).map_err(|e| e.to_string())?;
    }
    for epoch in &observations.epochs {
        for sat_rec in &epoch.satellites {
            for field in &sat_rec.fields {
                insert_obs_field(tx, project_id, input_version_id, epoch, sat_rec, field)?;
            }
        }
    }
    for diagnostic in observations.diagnostics.iter().chain(status.diagnostics.iter()) {
        tx.execute("insert into diagnostics(id,project_id,input_version_id,line_no,severity,code,message,raw_line) values(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![stable_id(&[input_version_id, "diagnostic", &diagnostic.line_no.to_string(), diagnostic.code]), project_id, input_version_id, diagnostic.line_no, diagnostic.severity, diagnostic.code, diagnostic.message, diagnostic.raw_line]).map_err(|e| e.to_string())?;
    }
    for epoch in &status.epochs {
        for field in &epoch.fields {
            tx.execute("insert into status_fields(id,project_id,input_version_id,epoch,t_seconds,sat,datum,key,raw_value,value,unit,raw_line,line_no) values(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![stable_id(&[input_version_id, "state", &epoch.epoch, &epoch.sat, &field.key]), project_id, input_version_id, epoch.epoch, epoch.t_seconds, epoch.sat, epoch.datum, field.key, field.raw, field.value, field.unit, epoch.raw_line, epoch.line_no]).map_err(|e| e.to_string())?;
        }
    }
    for clock in &status.clock_events {
        tx.execute("insert into clock_events(id,project_id,input_version_id,epoch,t_seconds,kind,delta_seconds,unit,raw_representation,line_no) values(?1,?2,?3,?4,?5,?6,?7,?8,'second',?9,?10)",
            params![stable_id(&[input_version_id, "clock", &clock.epoch]), project_id, input_version_id, clock.epoch, clock.t_seconds, clock.kind, clock.delta_seconds, clock.raw, clock.line_no]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn insert_obs_field(
    tx: &rusqlite::Transaction,
    project_id: &str,
    input_version_id: &str,
    epoch: &crate::model::ParsedEpoch,
    sat_rec: &crate::model::ParsedSatellite,
    field: &RawField,
) -> Result<(), String> {
    let id = stable_id(&[input_version_id, "obs", &epoch.epoch, &sat_rec.sat, &field.code]);
    tx.execute(
        "insert into obs_fields(id,project_id,input_version_id,epoch,t_seconds,sat,code,kind,raw_value,value,unit,lli,status,epoch_raw_line,field_raw_line,line_no) values(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        params![id, project_id, input_version_id, epoch.epoch, epoch.t_seconds, sat_rec.sat, field.code, crate::model::obs_kind(&field.code), field.raw, field.value, field.unit, field.lli, field.status, epoch.raw_line, sat_rec.raw_line, sat_rec.line_no],
    ).map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
fn compute_and_insert_run(
    tx: &rusqlite::Transaction,
    project_id: &str,
    input_version_id: &str,
    run_id: &str,
    seq: i64,
    observation_text: &str,
    _status_text: &str,
    params: &RuleParams,
    repairs: &[Repair],
    event_id: Option<String>,
) -> Result<(), String> {
    let created_at = run_created_at(tx, event_id.as_deref())?;
    tx.execute("insert into runs(id,project_id,seq,event_id,input_version_id,algorithm_version,parameter_fingerprint,params_json,created_at) values(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![run_id, project_id, seq, event_id, input_version_id, ALGORITHM_VERSION, engine::parameter_fingerprint(params), serde_json::to_string(params).unwrap(), created_at]).map_err(|e| e.to_string())?;
    let observations = load_observations(tx, input_version_id)?;
    let frequencies = load_frequencies(tx, input_version_id)?;
    let clocks = load_clocks(tx, input_version_id)?;
    let result = engine::run(engine::EngineInput {
        project_id,
        input_version_id,
        observations: &observations,
        frequencies: &frequencies,
        clocks: &clocks,
        params,
        accepted_repairs: repairs,
    });
    insert_derived(tx, run_id, project_id, input_version_id, result)?;
    let _ = observation_text;
    Ok(())
}

fn run_created_at(tx: &rusqlite::Transaction, event_id: Option<&str>) -> Result<String, String> {
    match event_id {
        Some(id) => tx.query_row("select created_at from events where id=?1", params![id], |r| r.get::<_, String>(0)).map_err(|e| e.to_string()),
        None => Ok("2026-09-25T09:00:01Z".into()),
    }
}

fn load_observations(tx: &rusqlite::Transaction, version_id: &str) -> Result<Vec<ObsField>, String> {
    let mut stmt = tx.prepare("select id,epoch,t_seconds,sat,code,value,lli,status from obs_fields where input_version_id=?1 order by t_seconds,sat,code").map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![version_id], |r| Ok(ObsField {
        field_id: r.get(0)?, epoch: r.get(1)?, t: r.get(2)?, sat: r.get(3)?, code: r.get(4)?, value: r.get(5)?, lli: r.get(6)?, status: r.get(7)?,
    })).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

fn load_frequencies(tx: &rusqlite::Transaction, version_id: &str) -> Result<Vec<Frequency>, String> {
    let mut stmt = tx.prepare("select system,carrier_code,mhz from frequencies where input_version_id=?1 order by system,carrier_code").map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![version_id], |r| Ok(Frequency { system: r.get(0)?, carrier_code: r.get(1)?, mhz: r.get(2)? })).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

fn load_clocks(tx: &rusqlite::Transaction, version_id: &str) -> Result<Vec<Clock>, String> {
    let mut stmt = tx.prepare("select epoch,t_seconds,kind,delta_seconds from clock_events where input_version_id=?1 order by t_seconds").map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![version_id], |r| Ok(Clock { epoch: r.get(0)?, t: r.get(1)?, kind: r.get(2)?, delta_seconds: r.get(3)? })).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

fn insert_derived(tx: &rusqlite::Transaction, run_id: &str, project_id: &str, input_version_id: &str, result: Value) -> Result<(), String> {
    for arc in result["arcs"].as_array().unwrap() {
        tx.execute("insert into arcs(id,run_id,project_id,sat,carrier_code,start_epoch,end_epoch,open,start_reason,end_reason,algorithm_version,parameter_fingerprint,input_version_id,stale) values(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,0)",
            params![arc["id"], run_id, project_id, arc["sat"], arc["carrier_code"], arc["start_epoch"], arc["end_epoch"], arc["open"].as_bool().unwrap_or(false), arc["start_reason"], arc["end_reason"], arc["algorithm_version"], arc["parameter_fingerprint"], input_version_id]).map_err(|e| e.to_string())?;
    }
    for item in result["boundaries"].as_array().unwrap() {
        tx.execute("insert into boundaries(id,run_id,project_id,sat,carrier_code,epoch,reason,strength,direct_source_field_id,algorithm_version,parameter_fingerprint,stale) values(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0)",
            params![item["id"], run_id, project_id, item["sat"], item["carrier_code"], item["epoch"], item["reason"], item["strength"], item["direct_source_field_id"], item["algorithm_version"], item["parameter_fingerprint"]]).map_err(|e| e.to_string())?;
    }
    for item in result["findings"].as_array().unwrap() {
        tx.execute("insert into findings(id,run_id,project_id,sat,epoch,band,kind,state,value,unit,message,algorithm_version,parameter_fingerprint,stale) values(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,0)",
            params![item["id"], run_id, project_id, item["sat"], item["epoch"], item["band"], item["kind"], item["state"], item["value"]["value"], item["value"]["unit"], item["message"], item["algorithm_version"], item["parameter_fingerprint"]]).map_err(|e| e.to_string())?;
    }
    for item in result["candidates"].as_array().unwrap() {
        tx.execute("insert into candidates(stable_identity,run_id,project_id,group_identity,sat,epoch,band,detector,hypothesis,proposed_cycles_l1,proposed_cycles_l2,residual_value,residual_unit,score,status,tie_group,rejection_reason,evidence_json,algorithm_version,parameter_fingerprint,direct_source_field_id,stale) values(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,0)",
            params![item["stable_identity"], run_id, project_id, item["group_identity"], item["sat"], item["epoch"], item["band"], item["detector"], item["hypothesis"], item["proposed_cycles_l1"], item["proposed_cycles_l2"], item["residual"]["value"], item["residual"]["unit"], item["score"], item["status"], item["tie_group"], item["rejection_reason"], item["evidence"].to_string(), item["algorithm_version"], item["parameter_fingerprint"], item["direct_source_field_id"]]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn active(tx: &rusqlite::Connection, project_id: &str) -> Result<Active, String> {
    let input_version_id: String = tx.query_row("select input_version_id from runs where project_id=?1 order by seq desc limit 1", params![project_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    let params_json: String = tx.query_row("select params_json from runs where project_id=?1 order by seq desc limit 1", params![project_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    let observation_text: String = tx.query_row("select original_text from source_files where input_version_id=?1 and role='rinex-observation-subset'", params![input_version_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    let status_text: String = tx.query_row("select original_text from source_files where input_version_id=?1 and role='satellite-status'", params![input_version_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    let run_id: String = tx.query_row("select id from runs where project_id=?1 order by seq desc limit 1", params![project_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    let mut repairs = Vec::new();
    let mut stmt = tx.prepare("select sat,epoch,band,cycles from repairs where run_id=?1 order by sat,epoch,band").map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![run_id], |r| Ok(Repair { sat: r.get(0)?, epoch: r.get(1)?, band: r.get(2)?, cycles: r.get(3)? })).map_err(|e| e.to_string())?;
    for repair in rows { repairs.push(repair.map_err(|e| e.to_string())?); }
    Ok(Active { input_version_id, observation_text, status_text, params: serde_json::from_str(&params_json).map_err(|e| e.to_string())?, repairs })
}

fn next_seq(tx: &rusqlite::Connection, project_id: &str) -> Result<i64, String> {
    tx.query_row("select coalesce(max(seq),0)+1 from events where project_id=?1", params![project_id], |r| r.get(0)).map_err(|e| e.to_string())
}
fn next_version_no(tx: &rusqlite::Connection, project_id: &str) -> Result<i64, String> {
    tx.query_row("select coalesce(max(version_no),0)+1 from input_versions where project_id=?1", params![project_id], |r| r.get(0)).map_err(|e| e.to_string())
}

fn mark_stale(tx: &rusqlite::Connection, project_id: &str, latest_run_id: &str) -> Result<(), String> {
    let previous_run: Option<String> = tx
        .query_row("select id from runs where project_id=?1 and id<>?2 order by seq desc limit 1", params![project_id, latest_run_id], |r| r.get(0))
        .ok();
    let Some(previous_run) = previous_run else { return Ok(()); };
    let latest_input: String = tx.query_row("select input_version_id from runs where id=?1", params![latest_run_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    let previous_input: String = tx.query_row("select input_version_id from runs where id=?1", params![previous_run], |r| r.get(0)).map_err(|e| e.to_string())?;

    tx.execute("update arcs set stale=1 where run_id=?1", params![previous_run]).ok();
    tx.execute("update boundaries set stale=1 where run_id=?1", params![previous_run]).ok();
    tx.execute("update findings set stale=1 where run_id=?1", params![previous_run]).ok();
    tx.execute("update candidates set stale=1 where run_id=?1", params![previous_run]).ok();

    if latest_input == previous_input {
        return Ok(());
    }

    let mut reused_arcs = Vec::new();
    {
        let mut stmt = tx.prepare("select id,sat,carrier_code,parameter_fingerprint,start_epoch,end_epoch,open,start_reason,coalesce(end_reason,'') from arcs where run_id=?1").map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![previous_run], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?, r.get::<_,String>(3)?, r.get::<_,String>(4)?, r.get::<_,String>(5)?, r.get::<_,i64>(6)?, r.get::<_,String>(7)?, r.get::<_,String>(8)?))).map_err(|e| e.to_string())?;
        for row in rows { reused_arcs.push(row.map_err(|e| e.to_string())?); }
    }
    for (id, sat, carrier, fingerprint, start_epoch, end_epoch, open, start_reason, end_reason) in reused_arcs {
        let same = tx.query_row("select exists(select 1 from arcs where run_id=?1 and id=?2 and parameter_fingerprint=?3 and start_epoch=?4 and end_epoch=?5 and open=?6 and start_reason=?7 and coalesce(end_reason,'')=?8)",
            params![latest_run_id, id, fingerprint, start_epoch, end_epoch, open, start_reason, end_reason], |r| r.get::<_,i64>(0)).unwrap_or(0) == 1;
        let band = carrier.trim_start_matches('L');
        let dependency_unchanged = signal_dependency_unchanged(tx, project_id, &previous_input, &latest_input, &sat, band)?;
        if same && dependency_unchanged {
            tx.execute("update arcs set stale=0 where run_id=?1 and id=?2", params![previous_run, id]).ok();
        }
    }

    let mut reused_boundaries = Vec::new();
    {
        let mut stmt = tx.prepare("select id,sat,carrier_code,direct_source_field_id from boundaries where run_id=?1").map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![previous_run], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?, r.get::<_,String>(3)?))).map_err(|e| e.to_string())?;
        for row in rows { reused_boundaries.push(row.map_err(|e| e.to_string())?); }
    }
    for (id, sat, carrier, old_field_id) in reused_boundaries {
        let band = carrier.trim_start_matches('L').to_string();
        let unchanged = tx.query_row(
            "select exists(select 1 from boundaries b join obs_fields o on o.id=b.direct_source_field_id join obs_fields n on n.input_version_id=?1 and n.epoch=o.epoch and n.sat=o.sat and n.code=o.code where b.run_id=?2 and b.id=?3 and ifnull(n.raw_value,'')=ifnull(o.raw_value,'') and ifnull(n.lli,-1)=ifnull(o.lli,-1) and n.status=o.status and not exists(select 1 from obs_fields x where x.id=?4 and x.code like 'S%'))",
            params![latest_input, previous_run, id, old_field_id],
            |r| r.get::<_,i64>(0),
        ).unwrap_or(0) == 1;
        let dependency_unchanged = signal_dependency_unchanged(tx, project_id, &previous_input, &latest_input, &sat, &band)?;
        if unchanged && dependency_unchanged {
            tx.execute("update boundaries set stale=0 where run_id=?1 and id=?2", params![previous_run, id]).ok();
        }
    }

    let mut reused_findings = Vec::new();
    {
        let mut stmt = tx.prepare("select id from findings where run_id=?1").map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![previous_run], |r| r.get::<_,String>(0)).map_err(|e| e.to_string())?;
        for row in rows { reused_findings.push(row.map_err(|e| e.to_string())?); }
    }
    for id in reused_findings {
        tx.execute("update findings set stale=0 where run_id=?1 and id in (select a.id from findings a join findings b on b.run_id=?2 and b.id=a.id where a.run_id=?1 and a.parameter_fingerprint=b.parameter_fingerprint and a.kind=b.kind and a.state=b.state and a.value=b.value and a.unit=b.unit and a.message=b.message)",
            params![previous_run, latest_run_id, id]).ok();
    }

    let mut reused_candidates = Vec::new();
    {
        let mut stmt = tx.prepare("select stable_identity,sat,band from candidates where run_id=?1").map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![previous_run], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?))).map_err(|e| e.to_string())?;
        for row in rows { reused_candidates.push(row.map_err(|e| e.to_string())?); }
    }
    for (id, sat, band) in reused_candidates {
        let bands: Vec<String> = band.split('/').map(|x| x.trim_start_matches('L').to_string()).collect();
        let dependency_unchanged = bands.iter().all(|band| signal_dependency_unchanged(tx, project_id, &previous_input, &latest_input, &sat, band).unwrap_or(false));
        let same = tx.query_row(
            "select exists(select 1 from candidates a join candidates b on b.run_id=?1 and b.stable_identity=a.stable_identity where a.run_id=?2 and a.parameter_fingerprint=b.parameter_fingerprint and a.status=b.status and a.score=b.score and ifnull(a.residual_value,-999)=ifnull(b.residual_value,-999) and ifnull(a.evidence_json,'')=ifnull(b.evidence_json,''))",
            params![latest_run_id, previous_run, id], |r| r.get::<_,i64>(0)).unwrap_or(0) == 1;
        if same && dependency_unchanged {
            tx.execute("update candidates set stale=0 where run_id=?1 and stable_identity=?2", params![previous_run, id]).ok();
        }
    }
    Ok(())
}

fn signal_dependency_unchanged(tx: &rusqlite::Connection, project_id: &str, old_version: &str, new_version: &str, sat: &str, band: &str) -> Result<bool, String> {
    let pattern = format!("_{band}");
    let count: i64 = tx.query_row(
        "select count(*) from obs_fields o join obs_fields n on n.project_id=o.project_id and n.epoch=o.epoch and n.sat=o.sat and n.code=o.code where o.project_id=?1 and o.input_version_id=?2 and n.input_version_id=?3 and o.sat=?4 and o.code like ?5 and (ifnull(o.raw_value,'')<>ifnull(n.raw_value,'') or ifnull(o.lli,-9)<>ifnull(n.lli,-9) or o.status<>n.status)",
        params![project_id, old_version, new_version, sat, format!("%{pattern}")], |r| r.get(0)).map_err(|e| e.to_string())?;
    Ok(count == 0)
}

fn project_json(conn: &Connection, project_id: &str) -> Result<Value, String> {
    let project = conn.query_row("select id,name,created_at,updated_at from projects where id=?1", params![project_id], |r| {
        Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"created_at":r.get::<_,String>(2)?,"updated_at":r.get::<_,String>(3)?}))
    }).map_err(|e| e.to_string())?;
    let active_input: String = conn.query_row("select input_version_id from runs where project_id=?1 order by seq desc limit 1", params![project_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    let active_run: String = conn.query_row("select id from runs where project_id=?1 order by seq desc limit 1", params![project_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    let mut events = Vec::new();
    {
        let mut stmt = conn.prepare("select id,seq,kind,created_at,payload from events where project_id=?1 order by seq").map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![project_id], |r| {
            let payload: String = r.get(4)?;
            Ok(json!({"id":r.get::<_,String>(0)?,"seq":r.get::<_,i64>(1)?,"kind":r.get::<_,String>(2)?,"created_at":r.get::<_,String>(3)?,"payload":serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null)}))
        }).map_err(|e| e.to_string())?;
        for row in rows { events.push(row.map_err(|e| e.to_string())?); }
    }
    let mut input_versions = Vec::new();
    {
        let mut stmt = conn.prepare("select id,version_no,parent_id,kind,source_hash,observation_sha,status_sha,created_event_id from input_versions where project_id=?1 order by version_no").map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![project_id], |r| Ok(json!({"id":r.get::<_,String>(0)?,"version_no":r.get::<_,i64>(1)?,"parent_id":r.get::<_,Option<String>>(2)?,"kind":r.get::<_,String>(3)?,"source_hash":r.get::<_,String>(4)?,"observation_sha":r.get::<_,String>(5)?,"status_sha":r.get::<_,String>(6)?,"created_event_id":r.get::<_,String>(7)?}))).map_err(|e| e.to_string())?;
        for row in rows { input_versions.push(row.map_err(|e| e.to_string())?); }
    }
    let raw_observations = raw_observations_json(conn, project_id, &active_input)?;
    let satellite_states = satellite_states_json(conn, project_id, &active_input)?;
    let mut runs = Vec::new();
    {
        let mut stmt = conn.prepare("select id,seq,event_id,input_version_id,algorithm_version,parameter_fingerprint,params_json,created_at from runs where project_id=?1 order by seq").map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![project_id], |r| {
            let params_text: String = r.get(6)?;
            Ok((r.get::<_,String>(0)?, r.get::<_,i64>(1)?, r.get::<_,Option<String>>(2)?, r.get::<_,String>(3)?, r.get::<_,String>(4)?, r.get::<_,String>(5)?, serde_json::from_str::<Value>(&params_text).unwrap_or(Value::Null), r.get::<_,String>(7)?))
        }).map_err(|e| e.to_string())?;
        for row in rows {
            let (id, seq, event_id, input_id, algorithm, fingerprint, params, created_at) = row.map_err(|e| e.to_string())?;
            runs.push(json!({
                "id": id,
                "seq": seq,
                "event_id": event_id,
                "input_version_id": input_id,
                "algorithm_version": algorithm,
                "parameter_fingerprint": fingerprint,
                "params": params,
                "created_at": created_at,
                "arcs": table_json(conn, "arcs", &id)?,
                "boundaries": table_json(conn, "boundaries", &id)?,
                "findings": table_json(conn, "findings", &id)?,
                "candidates": table_json(conn, "candidates", &id)?,
                "repairs": table_json(conn, "repairs", &id)?,
                "decisions": table_json(conn, "decisions", &id)?,
            }));
        }
    }
    Ok(json!({
        "project": project,
        "active_run_id": active_run,
        "active_input_version_id": active_input,
        "events": events,
        "input_versions": input_versions,
        "raw": raw_observations,
        "satellite_states": satellite_states,
        "runs": runs,
    }))
}

fn raw_observations_json(conn: &Connection, project_id: &str, version_id: &str) -> Result<Value, String> {
    let mut source_files = table_where(conn, "source_files", "project_id=?1 and input_version_id=?2", params![project_id, version_id])?;
    for file in source_files.iter_mut() {
        file.as_object_mut().unwrap().remove("project_id");
        file.as_object_mut().unwrap().remove("input_version_id");
    }
    let frequencies = table_where(conn, "frequencies", "project_id=?1 and input_version_id=?2", params![project_id, version_id])?;
    let fields = table_where(conn, "obs_fields", "project_id=?1 and input_version_id=?2", params![project_id, version_id])?;
    let diagnostics = table_where(conn, "diagnostics", "project_id=?1 and input_version_id=?2", params![project_id, version_id])?;
    Ok(json!({"source_files": source_files, "frequencies": frequencies, "fields": fields, "diagnostics": diagnostics}))
}

fn satellite_states_json(conn: &Connection, project_id: &str, version_id: &str) -> Result<Value, String> {
    Ok(json!({
        "fields": table_where(conn, "status_fields", "project_id=?1 and input_version_id=?2", params![project_id, version_id])?,
        "clock_events": table_where(conn, "clock_events", "project_id=?1 and input_version_id=?2", params![project_id, version_id])?,
    }))
}

fn table_json(conn: &Connection, table: &str, run_id: &str) -> Result<Vec<Value>, String> {
    table_where(conn, table, "run_id=?1", params![run_id])
}

fn table_where(conn: &Connection, table: &str, clause: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Vec<Value>, String> {
    let allowed = ["source_files","frequencies","obs_fields","status_fields","clock_events","diagnostics","arcs","boundaries","findings","candidates","repairs","decisions"];
    if !allowed.contains(&table) {
        return Err("invalid table".into());
    }
    let sql = format!("select * from {table} where {clause} order by 1,2,3,4,5");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let columns: Vec<String> = stmt.columns().map_err(|e| e.to_string())?.map(|c| c.name().to_string()).collect();
    let rows = stmt.query_map(params, |row| {
        let mut object = serde_json::Map::new();
        for (index, name) in columns.iter().enumerate() {
            let value = match row.get_ref(index) {
                Ok(rusqlite::types::ValueRef::Null) => Value::Null,
                Ok(rusqlite::types::ValueRef::Integer(v)) => json!(v),
                Ok(rusqlite::types::ValueRef::Real(v)) => json!(v),
                Ok(rusqlite::types::ValueRef::Text(v)) => json!(String::from_utf8_lossy(v).to_string()),
                Ok(rusqlite::types::ValueRef::Blob(v)) => json!(format!("<blob:{}>", v.len())),
                Err(_) => Value::Null,
            };
            if name == "open" || name == "stale" {
                if let Value::Number(n) = &value {
                    object.insert(name.clone(), json!(n.as_i64() == Some(1)));
                    continue;
                }
            }
            if name == "evidence_json" {
                object.insert("evidence".to_string(), serde_json::from_str(value.as_str().unwrap_or("null")).unwrap_or(Value::Null));
            } else if name == "params_json" {
                object.insert(name.clone(), value);
            } else {
                object.insert(name.clone(), value);
            }
        }
        Ok(Value::Object(object))
    }).map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for row in rows { result.push(row.map_err(|e| e.to_string())?); }
    Ok(result)
}

fn export_json(conn: &Connection, project_id: &str) -> Result<Value, String> {
    let tables = ["projects","events","input_versions","source_files","frequencies","obs_fields","status_fields","clock_events","diagnostics","rules","runs","arcs","boundaries","findings","candidates","repairs","decisions"];
    let mut payload = serde_json::Map::new();
    payload.insert("format".into(), json!("carrier-break-station-archive-v1"));
    payload.insert("exported_at".into(), json!("2026-09-25T00:00:00Z"));
    let mut data = serde_json::Map::new();
    for table in tables {
        data.insert(table.into(), json!(table_where(conn, table, "project_id=?1", params![project_id])?));
    }
    payload.insert("data".into(), Value::Object(data));
    Ok(Value::Object(payload))
}

fn import_json(tx: &rusqlite::Transaction, bundle: Value) -> Result<(), String> {
    if bundle["format"] != "carrier-break-station-archive-v1" {
        return Err("unsupported archive format".into());
    }
    let data = bundle.get("data").ok_or("archive missing data")?;
    let project = data["projects"].as_array().ok_or("archive missing projects")?;
    if project.len() != 1 {
        return Err("archive must contain exactly one project".into());
    }
    let order = ["projects","events","input_versions","source_files","frequencies","obs_fields","status_fields","clock_events","diagnostics","rules","runs","arcs","boundaries","findings","candidates","repairs","decisions"];
    for table in order {
        let rows = data.get(table).and_then(Value::as_array).cloned().unwrap_or_default();
        for row in rows {
            insert_json_row(tx, table, row)?;
        }
    }
    Ok(())
}

fn insert_json_row(tx: &rusqlite::Transaction, table: &str, row: Value) -> Result<(), String> {
    let columns: Vec<String> = row.as_object().ok_or("row must be an object")?.keys().cloned().collect();
    let values: Vec<Value> = columns.iter().map(|column| row[column].clone()).collect();
    let placeholders = (1..=columns.len()).map(|index| format!("?{index}")).collect::<Vec<_>>().join(",");
    let sql = format!("insert or replace into {table}({}) values({placeholders})", columns.join(","));
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    for value in values {
        params.push(match value {
            Value::Null => Box::new(rusqlite::types::Null),
            Value::Bool(v) => Box::new(v as i64),
            Value::Number(v) => Box::new(v.as_f64().unwrap_or(0.0)),
            Value::String(v) => Box::new(v),
            other => Box::new(other.to_string()),
        });
    }
    let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|x| x.as_ref()).collect();
    tx.execute(&sql, refs.as_slice()).map_err(|e| format!("import {table}: {e}"))?;
    Ok(())
}
