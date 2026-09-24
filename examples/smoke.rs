use cycle_slip_bench::{case, engine, model::Params, parser};
fn params_for(v: u32) -> Params {
    // v1 uses default (gf/mw 0.10... actually Params::default); v3.. uses v2 (0.15)
    if v == 1 {
        Params { gf_sigma_m: 0.10, mw_sigma_m: 0.10, ..Params::default() }
    } else {
        Params { gf_sigma_m: 0.15, mw_sigma_m: 0.15, ..Params::default() }
    }
}
fn main() {
    for v in [1u32, 3, 4, 5, 6, 7] {
        let txt = case::file_for_version(v).unwrap();
        let f = parser::parse(txt);
        let p = params_for(v);
        let d = engine::DecisionState::new();
        let view = engine::compute(engine::ComputeInput {
            file: &f, params: &p, input_fingerprint: "x".into(), decisions: &d,
        });
        println!("== v{v}: {} findings", view.findings.len());
        for fd in &view.findings {
            match fd.kind.as_str() {
                "cycle_slip" => {
                    let r = fd.residuals.as_ref().unwrap();
                    println!(" SLIP {} {} gf={:.4} mw={:.4} td1={:.3} td2={:.3} irr={}",
                        fd.satellite, fd.epoch,
                        r.gf_jump_m.unwrap_or(f64::NAN), r.mw_jump_m.unwrap_or(f64::NAN),
                        r.td_cycles_l1.unwrap_or(f64::NAN), r.td_cycles_l2.unwrap_or(f64::NAN), r.irregular);
                    for c in fd.candidates.iter() {
                        println!("    cand ({},{}) score={:.3} legal={} td_pass={}", c.n1,c.n2,c.score,c.legal,c.td_passes);
                    }
                    println!("    td_pick={:?} unique={:?}", fd.td_pick, fd.unique_pair);
                }
                "clock_jump" => println!(" CLOCK {} common={:.2}m explicit_us={:?}", fd.epoch, fd.clock_event_m.unwrap_or(0.), fd.clock_event_us),
                "lli_only" => println!(" LLI {} {} status={}", fd.satellite, fd.epoch, fd.status),
                "frequency_missing" => println!(" MISSING {} {} {}", fd.satellite, fd.band, fd.epoch),
                _ => {}
            }
        }
    }
}
