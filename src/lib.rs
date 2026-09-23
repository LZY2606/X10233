pub mod clock;
pub mod db;
pub mod detect;
pub mod engine;
pub mod error;
pub mod model;
pub mod parser;
pub mod cases;
pub mod service;
pub mod units;
pub mod util;
pub mod web;

pub const ALGO_VERSION: &str = "cycle-slip-core/v1.0.0";
pub const FIXED_SEED_MS: i64 = 1_700_000_000_000;
pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;
