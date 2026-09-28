//! 构建期把应用图标写进可执行文件的 PE 资源段。
//!
//! 资源编译器（`rc.exe`）的产物经 `cargo:rustc-link-arg-bins` 交给链接器，
//! 图标从此**烘焙进二进制**：产物没有多出任何伴随文件，运行期也没有新增依赖。
//!
//! # 为什么不能照抄上游示例
//!
//! `embed-resource` 的文档推荐 `compile(..).manifest_optional().unwrap()`。但在 MSVC 下
//! `windows_msvc::ResourceCompiler::is_supported()` 恒返回 `None`（即"总是支持"），
//! 于是「压根找不到 `rc.exe`」在 `compile_impl` 里同样被归入 `Failed`；
//! 而 `manifest_optional()` 只把 `NotWindows` / `Ok` / `NotAttempted` 放行。
//! 两者一叠加，`.unwrap()` 会让**没装 Windows SDK 的机器连 `cargo build` 都跑不过**。
//!
//! 因此这里在编译前自行探测一次编译器：探不到就只告警、跳过图标，构建照常完成。
//! 图标是装饰性资源，不该成为构建前提。

use std::path::Path;

fn main() {
    // 资源在 workspace 根，不在本 crate 内。用 CARGO_MANIFEST_DIR 拼绝对路径，
    // 不依赖构建脚本的工作目录。
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/app-icon/windows");
    let rc_file = assets.join("icon.rc");

    // 构建脚本默认不跟踪文件内容；不登记这里，改完图标不会重新嵌入。
    for tracked in [&rc_file, &assets.join("icon.ico")] {
        println!("cargo:rerun-if-changed={}", tracked.display());
    }
    // 探测结果同时取决于这些环境变量与 SDK 安装位置。不登记的话，用户为了让构建通过
    // 而设置 `RC` 之后，Cargo 不会重跑本脚本 —— 表现为「改了环境变量却毫无效果」，
    // 直到有人手动 touch 一次资源文件。
    for key in rc_override_keys() {
        println!("cargo:rerun-if-env-changed={key}");
    }

    // 非 Windows 主机没有 PE 资源可嵌（跨平台编译到 Windows 目标也一样：
    // 该分支判断的是构建机，而 `rc.exe` 无法在非 Windows 上运行）。
    if !cfg!(windows) {
        return;
    }

    if !rc_available() {
        println!(
            "cargo:warning=未找到 rc.exe（Windows SDK），已跳过应用图标嵌入；\
             exe 仍可正常构建与运行，只是显示系统默认图标。"
        );
        return;
    }

    // `icon.rc` 里的文件名是相对路径，而 rc.exe 的搜索基准是**本 crate 根**
    // （不是 .rc 所在目录），所以必须显式把资源目录加进 `/I` 搜索路径。
    match embed_resource::compile(&rc_file, embed_resource::ParamsIncludeDirs([&assets])) {
        embed_resource::CompilationResult::NotWindows => {}
        embed_resource::CompilationResult::Ok => {}
        embed_resource::CompilationResult::NotAttempted(reason) => {
            println!("cargo:warning=未嵌入应用图标：{reason}");
        }
        embed_resource::CompilationResult::Failed(reason) => {
            // 能走到这里说明 rc.exe 确实存在，失败就是资源脚本本身有问题。
            // 静默产出「没有图标的 exe」比构建失败更难排查，所以这里必须中断。
            println!("cargo::error=应用图标资源编译失败：{reason}");
            std::process::exit(1);
        }
    }
}

/// `rc.exe` 的覆盖环境变量名，查找顺序与 `embed-resource` 内部一致。
fn rc_override_keys() -> [String; 3] {
    let target = std::env::var("TARGET").unwrap_or_default();
    [
        format!("RC_{target}"),
        format!("RC_{}", target.replace('-', "_")),
        "RC".to_string(),
    ]
}

/// `rc.exe` 是否可用。
///
/// 只做存在性探测，真正的调用仍交给 `embed-resource` —— 避免「怎么找 SDK」这件事
/// 在两处各自实现一遍，然后随着时间逐渐产生分歧。
fn rc_available() -> bool {
    if rc_override_keys()
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()))
    {
        return true;
    }

    embed_resource::find_windows_sdk_tool("rc.exe").is_some()
}
