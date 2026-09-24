//! 载波断点台：本地优先的 GNSS 载波相位 / 周跳候选审阅系统。
//!
//! 核心原则：
//! - 原始输入（RINEX 观测历元与卫星状态）一经提交不可覆盖，修订以新版本 + 父版本表达；
//! - 派生值都带算法版本、参数指纹与直接来源；
//! - 电离层组合 / Melbourne-Wubbena / 时间差分给出各自候选，没有任何检测器是真值；
//! - 整机钟跳与单星周跳分开建模，修复只存偏移层，不改原始观测；
//! - 候选、拒绝原因、误差与未知状态都是一等数据；
//! - 局部输入变化只让依赖它的结果过期，过期结果仍可查看但不能冒充当前结果。

pub mod casepack;
pub mod db;
pub mod decision;
pub mod detect;
pub mod engine;
pub mod export;
pub mod rinex;
pub mod web;
