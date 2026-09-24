//! Virtual-time clock abstraction: tests use a fixed clock, the server uses
//! the system clock. No wall-clock leakage into the state machine.

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Fixed, manually advanced clock for deterministic tests.
pub struct FixedClock(pub std::sync::atomic::AtomicI64);
impl FixedClock {
    pub fn new(ms: i64) -> Self {
        FixedClock(std::sync::atomic::AtomicI64::new(ms))
    }
    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
    }
}
impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Days from civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Parse a minimal ISO-8601 epoch (`YYYY-MM-DDTHH:MM:SS[.fff][Z]`).
/// Returns milliseconds since Unix epoch, or None if unparseable; callers
/// keep the raw string and attach a diagnostic instead of dropping it.
pub fn parse_epoch_ms(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('Z').trim_end_matches('z');
    let s = s.replacen(' ', "T", 1);
    let mut it = s.splitn(2, 'T');
    let date = it.next()?;
    let time = it.next()?;
    let d: Vec<&str> = date.split('-').collect();
    if d.len() != 3 {
        return None;
    }
    let (y, mo, da) = (
        d[0].parse::<i64>().ok()?,
        d[1].parse::<i64>().ok()?,
        d[2].parse::<i64>().ok()?,
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&da) {
        return None;
    }
    let t: Vec<&str> = time.split(':').collect();
    if t.len() != 3 {
        return None;
    }
    let (hh, mm) = (t[0].parse::<i64>().ok()?, t[1].parse::<i64>().ok()?);
    let sec_raw = t[2];
    let (sec, frac_ms) = match sec_raw.split_once('.') {
        Some((a, b)) => {
            let frac = format!("{:0<3}", &b[..b.len().min(3)]);
            (a.parse::<i64>().ok()?, frac.parse::<i64>().ok()?)
        }
        None => (sec_raw.parse::<i64>().ok()?, 0),
    };
    if hh > 24 || mm > 59 || sec > 60 {
        return None;
    }
    let days = days_from_civil(y, mo, da);
    Some((((days * 24 + hh) * 60 + mm) * 60 + sec) * 1000 + frac_ms)
}
