//! cycle_slip_bench — 本地优先 GNSS 载波相位周跳复核工作台。
//!
//! 核心原则：
//! - 原始输入（伪距/载波/Doppler/LLI/信噪比/钟跳事件/频率编号/观测码）按历元原样保存，不可覆盖；
//!   修订以新版本 + 父版本关系表达。
//! - 电离层组合、Melbourne-Wübbena、时间差分是三个并列检测器，各自产出候选，
//!   任何单一检测器都不作为真值；候选、拒绝原因、未知状态都是一等数据。
//! - 整机钟跳与单星周跳严格分开；修复只存偏移层（decision overlay），不改原始观测。
//! - 派生值带算法版本、参数指纹与直接来源；局部输入变化只使依赖它的结果过期，
//!   过期结果仍可查看（stale 标记），不能冒充当前结果。
//! - 事件溯源：导入/参数修订/人工裁定/撤销都是事件；回到任意旧事件可恢复当时的
//!   依赖版本与可见结论（as-of 视图）。

pub mod clock;
pub mod domain;
pub mod events;
pub mod export;
pub mod ids;
pub mod ingest;
pub mod model;
pub mod server;
pub mod store;

/// 派生算法版本。任何组合/候选/弧段计算逻辑变更时递增。
pub const ALGO_VERSION: &str = "csb-algo-1";
