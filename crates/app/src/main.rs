// 该属性必须在任何条目之前：release 产物使用 Windows GUI 子系统，避免弹出控制台黑窗。
// debug 保留控制台，便于直接看到 tracing 日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod bootstrap;

fn main() {
    bootstrap::run();
}
