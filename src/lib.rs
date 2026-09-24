//! 载波断点台 (Carrier-Break Station)
//!
//! 本地优先的 GNSS 载波相位 / 周跳候选审阅工作站。
//!
//! 设计原则见 `docs/design.md`：
//! - 原始观测按历元原样保存（raw-preserving），只追加，永不覆盖；
//! - 派生值（弧段、候选、得分、修复偏移）携带算法版本、参数指纹与直接来源；
//! - 三个检测器（电离层/几何无关组合、Melbourne-Wübbena、时间差分）各自独立，
//!   任一检测器都不是“真值”；
//! - 接收机钟跳与单星周跳分离；修复只落在偏移层，绝不回写载波；
//! - 局部输入变化只令依赖它的派生结果过期，过期结果可查看但不能冒充当前结果。

pub mod vclock;
pub mod digest;
pub mod domain;
pub mod rinex;
pub mod detectors;
pub mod events;
pub mod store;
pub mod api;
