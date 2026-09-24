//! 领域核心：时间、坐标基准、单位、频点/观测码、观测量。
//!
//! 约定（单位与坐标基准随数据携带）：
//! - 伪距 `range_m`：米（RINEX 原始单位本就是米）；
//! - 载波 `phase_cycles`：周（RINEX 原始单位为周）；
//! - Doppler `doppler_hz`：Hz（按 RINEX 3 约定，卫星趋近接收机为正）；
//! - LLI / SSI：无量纲标志位，原值保留；
//! - 卫星坐标：ITRF，米；速度：ITRF，米/秒；钟差：秒。
//! - GLONASS 频点依赖“频率编号 k”：f = f0 + k*Δf（L1: 562500 kHz, Δ=562.5 kHz）。

pub mod calendar;
pub mod units;
pub mod signal;
pub mod obs;
pub mod series;

pub use units::Unit;
pub use signal::{SignalDef, Constellation, ObsCode, frequency_table, signal_for_code};
pub use obs::{EpochObs, FrequencyObs, ClockEvent};
pub use series::{Point, Arc, ArcId, ObservationTable};
