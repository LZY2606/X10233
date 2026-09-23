pub mod clock;
pub mod error;
pub mod ids;
pub mod units;
pub mod parser;
pub mod model;
pub mod store;
pub mod engine;
pub mod service;
pub mod case;
pub mod api;

pub use error::{DeskError, Result};
pub use service::Desk;
