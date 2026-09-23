//! 载波断点台 (carrier-break-desk)
//!
//! 本地优先的 GNSS 载波相位周跳候选审阅台：
//! 原始观测（伪距/载波/Doppler/LLI/信噪比/钟跳/频率编号/观测码）按历元原样保存，
//! 派生层（电离层组合、Melbourne-Wubbena、时间差分、弧段、候选）带算法版本、
//! 参数指纹与直接来源；局部输入变化只让依赖它的结果产生新版本，旧版本仍可查看。

pub mod api;
pub mod clock;
pub mod engine;
pub mod ids;
pub mod model;
pub mod parser;
pub mod store;
pub mod units;
pub mod web;

pub const ALGO_VERSION: &str = "carrier-break-desk-arc-1.0.0";
