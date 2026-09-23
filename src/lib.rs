//! 载波断点台：GNSS 周跳候选、多频组合与弧段修订史。
//!
//! 本地优先的审阅工作站：导入 RINEX 风格观测历元（RNXL 文本），
//! 原样保存伪距/载波/Doppler/LLI/信噪比/钟跳事件/频率编号/观测码，
//! 在派生层计算弧段、组合残差与周跳候选，人工裁定以事件形式入史。

pub mod model;
pub mod rnxl;
pub mod store;
pub mod detect;
pub mod engine;
pub mod archive;
pub mod casepack;
pub mod api;
pub mod web;

/// 算法版本：任何检测/分段逻辑变更都必须提升，派生值随指纹失效。
pub const ALGO_VERSION: &str = "1.0.0";

/// GPS 标称频率（Hz），用于 Melbourne-Wübbena 与电离层组合。
pub const F1_HZ: f64 = 1_575_420_000.0;
pub const F2_HZ: f64 = 1_227_600_000.0;
