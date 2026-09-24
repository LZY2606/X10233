#[test]
fn dbg_dups() {
    let p = carrier_breakpoint::parser::parse(carrier_breakpoint::cases::BASELINE_CBF, "in_x");
    let o = p.observations.iter().find(|o| o.code=="L1").unwrap();
    println!("rawbytes={:?} unitbytes={:?}", o.raw_value.as_bytes(), o.raw_unit.as_bytes());
    println!("split={:?}", {
        let token=&o.raw_value;
        let idx = token.rfind(|c: char| c.is_ascii_digit() || c=='.' || c=='e' || c=='E' || c=='+' || c=='-').map(|x|x+1).unwrap_or(token.len());
        token.split_at(idx)
    });
}
