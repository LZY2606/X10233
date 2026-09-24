//! Bundled offline case pack: "载波断点台".
pub const CASE_ID: &str = "carrier-break-station";

pub fn steps_json() -> &'static str {
    include_str!("../cases/carrier-break-station/steps.json")
}

pub fn file_for_version(v: u32) -> Option<&'static str> {
    match v {
        1 => Some(include_str!("../cases/carrier-break-station/input-v1.txt")),
        3 => Some(include_str!("../cases/carrier-break-station/input-v3.txt")),
        4 => Some(include_str!("../cases/carrier-break-station/input-v4.txt")),
        5 => Some(include_str!("../cases/carrier-break-station/input-v5.txt")),
        6 => Some(include_str!("../cases/carrier-break-station/input-v6.txt")),
        7 => Some(include_str!("../cases/carrier-break-station/input-v7.txt")),
        _ => None,
    }
}
