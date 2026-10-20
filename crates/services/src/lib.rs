//! 服务层：把平台能力加工成 UI 可直接消费的数据流。
//!
//! 两条硬性约束：
//! 1. **不得阻塞 UI 线程**：目录扫描、容量查询、图标提取一律在 GPUI 的
//!    `BackgroundExecutor` 上执行（不引入 tokio，沿用 GPUI 自带的 smol/async-task）。
//! 2. **结果必须分批回传并按 `generation` 校验**：过期批次直接丢弃，
//!    避免快速切换目录时出现结果错乱与无谓重绘。
//!
//! 写操作（新建 / 复制 / 移动 / 重命名 / 移入回收站）在 [`ops`] 与 [`clipboard`] 里，
//! 它们受同一条约束：真实调用必须发生在后台线程上——复制一个大目录是秒级操作，
//! 放在 UI 线程上就是一次肉眼可见的卡死。

pub mod clipboard;
pub mod config;
pub mod directory;
pub mod icon;
pub mod ops;
pub mod session;
pub mod watch;
