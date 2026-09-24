//! 载波断点台 (Carrier Break Station)
//!
//! 本地优先的 GNSS 载波相位 / 周跳审阅系统：
//! 原始观测不可变（raw layer），派生结果带算法版本、参数指纹与直接来源
//! （derived layer），所有人工裁定与冲突解决都写入只追加事件历史。

pub mod clock;
pub mod domain;
pub mod engine;
pub mod export;
pub mod parser;
pub mod physics;
pub mod storage;
pub mod webapi;

pub mod cases;

pub use clock::{Clock, DeterministicClock};
pub use engine::Engine;
