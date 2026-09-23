//! 服务层：把平台能力加工成 UI 可直接消费的数据流。
//!
//! 两条硬性约束：
//! 1. **不得阻塞 UI 线程**：目录扫描、容量查询、图标提取一律在 GPUI 的
//!    `BackgroundExecutor` 上执行（不引入 tokio，沿用 GPUI 自带的 smol/async-task）。
//! 2. **结果必须分批回传并按 `generation` 校验**：过期批次直接丢弃，
//!    避免快速切换目录时出现结果错乱与无谓重绘。

pub mod config;
pub mod directory;
pub mod icon;
pub mod watch;
