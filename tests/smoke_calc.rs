use carrier_breakpoint::cases;
use carrier_breakpoint::parser;
use carrier_breakpoint::params::DetectorParams;
use carrier_breakpoint::vtime::FixedClock;
use carrier_breakpoint::Store;

fn fresh() -> Store {
    Store::open_memory(Box::new(FixedClock::seeded())).unwrap()
}

#[test]
fn baseline_produces_expected_families() {
    let store = fresh();
    let parsed = parser::parse(cases::BASELINE_CBF, "placeholder");
    // commit_import computes batch id internally; parse with matching id after first call impossible,
    // so commit_import parses raw text itself: we pass raw text.
    let receipt = store.commit_import_raw("baseline", cases::BASELINE_CBF);
    assert_eq!(receipt.diagnostics, 0);
    let view = store.recompute_tip();
    println!("epochs={}", view.epochs.len());
    println!("diag={:?}", view.diagnostics);
    println!("stats={}", view.statistics.len());
    println!("cands={}", view.candidates.len());
    for c in &view.candidates {
        println!(
            "CAND {} sat={} band={} at={} off={:?}/{:?} score={:.3} suggest_reject={} amb={}",
            c.detector, c.sat, c.band, c.at_epoch, c.offset_cycles, c.offset_b_cycles,
            c.score, c.suggest_reject, c.ambiguity_n
        );
    }
    for s in view.statistics.iter().filter(|s| s.detector!="CLOCK") {
        println!("STAT {} {} {} val={:?} flag={:?}", s.detector, s.sat, s.band, s.value, s.flag);
    }
    assert!(view.epochs.iter().any(|e| e.irregular_gap) || view.epochs.iter().any(|e| e.declared_clock_delta_ns.is_some()));
}
