use std::sync::Arc;
use cycle_slip_bench::clock::VirtualClock;
use cycle_slip_bench::ingest;
use cycle_slip_bench::store::Store;

#[test]
fn debug_print() {
    let dir = tempfile::tempdir().unwrap();
    let clk = VirtualClock::new(1_700_000_000_000);
    let store = Store::open(&dir.path().join("d.sqlite"), clk).unwrap();
    let pid = store.create_project("bench").unwrap();
    let text = std::fs::read_to_string("cases/carrier-break-bench/bench.sheet.json").unwrap();
    let out = ingest::parse_any(&pid, None, "bench.sheet.json", &text).unwrap();
    store.commit_batch(&pid, out).unwrap();
    let hv = store.head(&pid).unwrap();
    for c in &hv.view.candidates {
        println!("CAND sv={} ep={} status={:?} lli={:?}", c.sv, c.epoch_raw, c.status, c.lli_flags);
        for v in &c.votes {
            println!("   {} resid={:?} unit={} n1={:?} n2={:?} score={:.3} {}",
                v.detector, v.residual, v.residual_unit, v.shift_cycles_b1, v.shift_cycles_b2, v.score, v.note);
        }
        for h in &c.hypotheses {
            println!("   HYP n1={:?} n2={:?} score={:.4} tied={}", h.shift_b1_cycles, h.shift_b2_cycles, h.total_score, h.tied);
        }
        for r in &c.rejections {
            println!("   REJ {} :: {}", r.reason_code, r.detail);
        }
    }
    println!("clock events: {}", hv.view.clock_events.len());
    for ce in &hv.view.clock_events {
        println!("  CLK {} {:?} jump={:?}", ce.epoch_raw, ce.event_type, ce.jump_seconds.value);
    }
    println!("arcs: {}", hv.view.arcs.len());
    for a in &hv.view.arcs {
        println!("  ARC {} b{} {}..{} [{}]", a.sv, a.band, a.start_epoch_raw, a.end_epoch_raw, a.boundary_reason);
    }
}
