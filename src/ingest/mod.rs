//! Import layer.
//!
//! Accepts a small, reproducible RINEX 3 observation subset and an explicit
//! JSON sheet. Pseudorange, carrier, Doppler, LLI, SNR, clock events,
//! frequency number and observation codes are stored verbatim per epoch.
//! Values that cannot be converted stay in place with diagnostics — they are
//! never dropped to keep plots tidy.

pub mod rinex;
pub mod sheet;

use crate::event::RawBatch;
use crate::model::Id;

/// Result of attempting one import. Structural problems return `Err` (nothing
/// is committed); per-row conversion problems are kept inside the batch.
#[derive(Debug)]
pub struct IngestOutcome {
    pub batch: RawBatch,
    pub warnings: Vec<String>,
}

/// Pick parser by content sniffing.
pub fn parse_any(
    project_id: &Id,
    parent_batch_id: Option<Id>,
    source_name: &str,
    text: &str,
) -> Result<IngestOutcome, String> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') {
        sheet::parse_sheet(project_id, parent_batch_id, source_name, text)
    } else {
        rinex::parse_rinex(project_id, parent_batch_id, source_name, text)
    }
}

/// GPS L-band frequencies in Hz by RINEX band digit (used for the mini set).
pub fn gps_frequency_hz(band: u8) -> Option<f64> {
    match band {
        1 => Some(1575.42e6),
        2 => Some(1227.60e6),
        5 => Some(1176.45e6),
        _ => None,
    }
}

/// RINEX SSI digit (1..9) mapped to dB-Hz. 0 = unknown.
/// Documented bands: n -> 6n+6 dB-Hz (5 -> 36, 9 -> 60).
pub fn ssi_to_dbhz(ssi: u8) -> Option<f64> {
    match ssi {
        0 => None,
        n => Some(6.0 * f64::from(n) + 6.0),
    }
}
