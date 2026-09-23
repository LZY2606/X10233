//! carrier-breakbench: 本地优先的 GNSS 载波相位周跳审阅台。
//!
//! 分层:
//! - raw 层: 导入的 RINEX 风格观测按历元原样保存, 不可覆盖, 修订以父子版本表达。
//! - derived 层: 规范化值/弧段/组合残差/候选, 全部携带算法版本、参数指纹与直接来源。
//! - decision 层: 人工裁定(接受/否定/调整/撤销), 冲突保留双方。
//! - event 层: 一切变化写入机器可读事件, 可回到任意事件重放当时结论。

pub mod clock;
pub mod model;
pub mod store;
pub mod parse;
pub mod engine;
pub mod casepack;
pub mod export;
pub mod web;
