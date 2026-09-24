//! Stable domain vocabulary. Raw representations are kept verbatim; only the
//! derived layer normalizes into documented SI units / coordinate datums.

pub const ALGO_VERSION: &str = "cbr-engine-1.0.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Code,
    Phase,
    Doppler,
    Snr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObsCode {
    pub system: char,
    pub kind: Kind,
    pub band: u8,
    pub attr: char,
}

impl ObsCode {
    pub fn parse(s: &str) -> Option<ObsCode> {
        let mut c = s.chars();
        let system = c.next()?;
        let kind_c = c.next()?;
        let band_c = c.next()?;
        let attr = c.next().unwrap_or(' ');
        let kind = match kind_c {
            'C' | 'P' => Kind::Code,
            'L' => Kind::Phase,
            'D' => Kind::Doppler,
            'S' => Kind::Snr,
            _ => return None,
        };
        let band = band_c.to_digit(10)? as u8;
        if !(1..=9).contains(&band) {
            return None;
        }
        Some(ObsCode {
            system,
            kind,
            band,
            attr,
        })
    }

    pub fn as_string(&self) -> String {
        format!("{}{}{}{}", self.system, self.kind_letter(), self.band, self.attr)
    }

    pub fn kind_letter(&self) -> char {
        match self.kind {
            Kind::Code => 'C',
            Kind::Phase => 'L',
            Kind::Doppler => 'D',
            Kind::Snr => 'S',
        }
    }
}

/// A value kept in two layers: `raw` is the exact token from the epoch line;
/// `norm` is the parsed quantity in `unit` (SI for the derived layer), if the
/// raw token was convertible. Values that cannot be converted stay in place
/// with a diagnostic instead of being dropped.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Quantity {
    pub raw: String,
    pub norm: Option<f64>,
    pub unit: String,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FrequencyDef {
    pub code: String,
    pub freq_hz: f64,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct EpochKey {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: f64,
}

impl EpochKey {
    pub fn label(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:06.3}",
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second
        )
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ObsCell {
    pub code: String,
    pub value: Quantity,
    pub lli: Option<u8>,
    pub snr_db_hz: Option<f64>,
    pub snr_raw: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EpochRecord {
    pub epoch: EpochKey,
    pub epoch_flag: i32,
    pub sat: String,
    pub clock_offset_s: Option<f64>,
    pub clock_offset_raw: String,
    pub cells: Vec<ObsCell>,
    pub comments: Vec<String>,
    pub source_line: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MarkerEvent {
    pub epoch: EpochKey,
    pub kind: String,
    pub text: String,
    pub source_line: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SatState {
    pub sat: String,
    pub datum: String,
    pub values: Vec<Quantity>,
    pub source_line: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParsedSubmission {
    pub marker_name: String,
    pub marker_number: String,
    pub receiver: String,
    pub antenna: String,
    pub frequencies: Vec<FrequencyDef>,
    pub records: Vec<EpochRecord>,
    pub markers: Vec<MarkerEvent>,
    pub sat_states: Vec<SatState>,
    pub header_lines: Vec<(usize, String, String)>,
}
