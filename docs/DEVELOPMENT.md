# 开发指南

> 环境要求、构建、打包、依赖与仓库结构，面向开发者。

## 环境要求

| 组件        | 本机实测版本                                | 说明                                                |
| ----------- | ------------------------------------------- | --------------------------------------------------- |
| Windows     | 10 / 11 x64                                 |                                                     |
| Rust        | 1.98.1 stable，host`x86_64-pc-windows-msvc` | `rust-toolchain.toml` 已钉 stable 通道              |
| MSVC        | VS 2022 Community，MSVC 14.44.35207         | 提供`cl.exe` / `link.exe`                           |
| Windows SDK | 10.0.26100.0                                | 提供`rc.exe` 与 `fxc.exe`（release 编译着色器必需） |

## 构建与运行

### 先选命令，再动手

**改完代码不需要每次都编译。** 按目的选命令，耗时差两个数量级（下表为本机实测墙钟时间）：

| 目的 | 命令 | 本机实测 |
| --- | --- | --- |
| 只确认类型 / 借用 / 命名有没有写错 | `cargo check -p fm-shell` | **3–8 s** |
| 看界面效果 | `cargo run`（或 `cargo build -p fm-app` 后运行产物） | 20–30 s |
| 跑单元测试 | `cargo test -p fm-shell` | 秒级 |
| 静态检查 | `cargo clippy -p fm-views -p fm-shell` | 6–10 s |
| **出可分发的产物** | `cargo build --release -p fm-app` | **3 分 07 秒 – 3 分 26 秒** |

release 之所以慢，是因为 `[profile.release]` 开了 `lto = "thin"` + `strip = true` + `opt-level = 3`。
因此**不要用 release 来「确认没写错」**——那是 `cargo check` 的活。release 只留给三件事：
验证 LTO / strip 下的真实产物、改过着色器、打安装包。

另外优先用 `-p <crate>` 而不是裸 `cargo build`：后者会连带编译整个 workspace。

### 常用命令

```bash
cargo check -p fm-shell        # 最快的「编译一下看看」
cargo run                      # debug：保留控制台，便于直接看 tracing 日志
cargo run -- D:\some\dir       # `--` 之后的第一个位置参数指定初始目录，缺省为用户主目录
cargo run -- computer          # 以「此电脑」（驱动器总览）为初始位置
cargo run -- 回收站             # 直接进入回收站（只读浏览）
cargo run -- --hotkey-host     # 只跑热键宿主（托盘 + Win+E），不启动界面
cargo build --release -p fm-app  # 可分发的单文件产物
cargo test --workspace         # 单元测试
```

文件夹接管的三个命令行入口（执行完即退出，不启动界面，适合脚本调用）：

```bash
pagefold.exe --install-folder-integration  # 安装右键菜单项 + 设为默认打开程序
pagefold.exe --restore-folder-integration  # 还原全部改动
pagefold.exe --folder-integration-status   # 打印当前状态（退出码 0）
```

**原生构建，不加 `--target`。** 不需要 VS Developer Prompt，也不需要手动把 SDK 的 bin 注入 `PATH`（原因见下节）。
可通过 `RUST_LOG` 覆盖日志级别，例如 `RUST_LOG=debug cargo run`。

### 构建前必须知道的三件事

**① 运行中的实例会锁住 exe——这是最常见的构建失败。**

Windows 不允许覆盖正在运行的映像，报错形如：

```
error: failed to remove file `...\target\debug\pagefold.exe`
Caused by: 拒绝访问。 (os error 5)
```

先结束进程再构建。注意**热键宿主是无窗口的后台进程**，在任务管理器里很容易漏掉：

```bash
taskkill /IM pagefold.exe /F        # 主窗口与热键宿主一起结束
```

宿主不需要手动重启：主程序每次启动都会 `ensure_hotkey_host()`（见 `bootstrap.rs`）。

**② 动过 `assets/app-icon/windows/icon.rc`、或补装了 Windows SDK，可能需要 `cargo clean -p fm-app`。**

Cargo 会缓存构建脚本的输出。若先在没装 SDK 的机器上构建过（只输出 warning、跳过图标嵌入），
之后补装 SDK 再 `cargo build` **不会**重新嵌入图标——因为 `build.rs` 登记的
`rerun-if-changed` 文件没变，脚本不重跑。只想验证图标时：

```bash
cargo clean -p fm-app && cargo build -p fm-app
```

**③ 改着色器必须用 release 验证。**

- **debug**：不编译着色器，运行期直接从 cargo registry cache 里读 `src/shaders.hlsl`。
  好处是改 HLSL 不必重编译，代价是**产物依赖 registry cache 不被清空**。
- **release**：构建期由 `fxc.exe` 编译成字节码并内嵌，产物自包含。

### 两个 workspace

`cargo test --workspace` 只覆盖根 workspace（`crates/*`）。图标生成器是**独立** workspace：

```bash
cargo test --manifest-path tools/icon-gen/Cargo.toml
```

安装包的构建方式见下一节。

## 打包安装程序

安装包由 [Inno Setup](https://jrsoftware.org/isinfo.php) 定义，脚本是 [`installer/files.iss`](./installer/files.iss)。

```bat
cargo build --release -p fm-app                 REM 1. 先出载荷
target\innosetup\ISCC.exe installer\files.iss  REM 2. 编译安装包
REM 产物：dist\Files-<版本>-x64-setup.exe
```

**Inno Setup 本体不需要安装。** 官方安装器支持免管理员的 portable 解包，解到 `target\innosetup`
即可（`target/` 与 `dist/` 都不入库）：

```bat
curl -L -o target\innosetup-6.7.3.exe https://github.com/jrsoftware/issrc/releases/download/is-6_7_3/innosetup-6.7.3.exe
target\innosetup-6.7.3.exe /PORTABLE=1 /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /DIR="C:\path\to\file-manager\target\innosetup"
```

> 该文件的 SHA-256 是 `9c73c3bae7ed48d44112a0f48e66742c00090bdb5bef71d9d3c056c66e97b732`，
> 与 winget 清单里 Inno Setup 6.7.3 记录的哈希一致，可据此校验。
> 中文界面文件 `installer/languages/ChineseSimplified.isl` 取自官方源码仓库
> （Inno Setup 官方发行版只带英文等 29 种语言，中文在「非官方翻译」之列），已固化入库，
> 构建不依赖网络。

### CI 自动发布（打 tag）

`.github/workflows/release.yml` 负责把上面两条命令搬到 GitHub Runner 上：

```bash
git tag v0.6.2 && git push origin v0.6.2
```

推送 tag 后自动执行：`cargo build --release --locked` → `cargo test --workspace --release --locked`
（发布门槛）→ portable 解包 Inno Setup → `ISCC /DAppVersion=… /DAppVersionQuad=…` →
上传 artifact → 用 `GITHUB_TOKEN` 创建 Release 并附上安装包与 `SHA256SUMS.txt`。

| 事项 | 说明 |
| --- | --- |
| 触发条件 | `push` 形如 `v*` 的 tag；也支持 Actions 页面手动 Run：填 `publish_tag` 则**重新发布该 tag**（更新说明与资产），留空则只编译并上传 artifact（试跑） |
| 版本号 | **以 tag 为准**：`vX.Y.Z` 或 `vX.Y.Z-<后缀>`（后者自动标 prerelease）。版本通过 ISPP 的 `/DAppVersion` 传给 `installer/files.iss`，脚本里用 `#ifndef` 兜底默认值 |
| 版本一致性 | `Cargo.toml` 与 `installer/files.iss` 的版本**不参与**发布产物命名，只影响本机构建。工作流会比对「tag / Cargo.toml / files.iss」三者，不一致时在作业摘要里给 warning 提醒，**不阻断发布** |
| 发布说明 | 由提交历史生成：按 `feat` / `fix` / `perf` / `refactor` / `docs` / `ci` 前缀分组，条目带 scope 与短 SHA 链接；顶部固定「下载与安装 + 校验 + 系统要求」，末尾给 `compare` 链接与升级注意。**不用** `--generate-notes`（本项目直接推 main、提交是中文的，自动生成只剩一句 Full Changelog） |
| 只有构建类改动 | 若区间内没有「新增 / 修复 / 性能 / 重构」条目，说明里会明写「本次发布只包含构建、文档与发布流程的调整」，避免用户对着一行 CI 记录猜更新内容 |
| 权限 | 只需要 `GITHUB_TOKEN`（工作流已声明 `contents: write`），无需额外 secret |
| 依赖镜像 | Runner 在境外，工作流会先删掉 `.cargo/config.toml`（USTC 镜像）让 cargo 回落 crates.io——这正是该文件头部写明的用法 |
| 安装包工具 | 与本地同一版本、同一 SHA-256 校验，解包到 `target/innosetup`，不装进系统 |
| 重复触发 | 同一 tag 的 Release 已存在时改为 `gh release edit`（更新标题与说明）+ `gh release upload --clobber`（覆盖资产）。因此**修正已发布版本的说明不需要删 tag 重打**：手动 Run 填上 `publish_tag` 即可 |
| 已知依赖 | Runner 需具备 VS 生成工具与 Windows SDK（`rc.exe` 编译图标资源、GPUI 的着色器工具）；`windows-latest` 自带 |

> 手动 Run 且未填 `publish_tag` 时，版本号是 `0.0.0-dev.<run_number>`，只用于给 artifact 起名；
> 此时的发布说明取最近 20 个提交，仅用于在 artifact 里预览格式。

发版清单（以 0.6.2 为例）：

```bash
# 1. 对齐版本：下面三处必须是同一个版本
#    - Cargo.toml            → workspace.package.version = "0.6.2"
#    - installer/files.iss   → AppVersion "0.6.2" / AppVersionQuad "0.6.2.0"
#    （工作流只提醒不一致，不会替你改；README 里的「当前版本」也顺手改一下）

# 2. 本地预检：全部通过再打 tag，别让失败发生在 CI 上
cargo clippy --workspace --all-targets     # 零告警
cargo test --workspace                     # 全绿（含 2 项 ignored 的手动测试不算在内）
cargo build --release -p fm-app            # 本机可运行的 exe

# 3. 本地打一次安装包（可选但值得）：验证 files.iss 与资源真的能编译
MSYS_NO_PATHCONV=1 target/innosetup/ISCC.exe \
  /DAppVersion=0.6.2 /DAppVersionQuad=0.6.2.0 installer/files.iss
# 产物：dist/Pagefold-0.6.2-x64-setup.exe
# （MSYS_NO_PATHCONV=1 必须加：否则 MSYS 会把 /DAppVersion 当成路径改写）

# 4. 提交、打 tag、推送
git commit -am "chore(release): v0.6.2"
git tag v0.6.2 && git push origin v0.6.2

# 5. 发布后核对三件事
#    - Release 里有 Pagefold-0.6.2-x64-setup.exe 与 SHA256SUMS.txt
#    - 发布说明的分组与条目完整（不是「Full Changelog」一句）
#    - 下载安装后，设置页底部显示的版本号是 0.6.2（取自 CARGO_PKG_VERSION）
```

安装包的名字与版本列**永远取自 tag**，因此即使漏改上面三处，发布出去的包也是对的；
那几处的用途是「本机构建时用哪个版本」与「设置页显示哪个版本」，漏改只会让本机产物、
界面版本号与发布产物对不上。

> 若发布后才发现说明或资产有问题：**不要删 tag 重打**（已下载的用户会指向不存在的版本）。
> 在 Actions 页面手动 Run 并填 `publish_tag=v0.6.2`，工作流会 `gh release edit` 更新说明、
> `upload --clobber` 覆盖资产。

安装包的行为约定：

| 项 | 取值 | 理由 |
|---|---|---|
| 安装范围 | 当前用户，`%LOCALAPPDATA%\Programs\Pagefold` | 单文件程序不写任何共享位置，装到 Program Files 只会平白要求管理员权限 |
| 提权 | 全程不弹 UAC | `PrivilegesRequired=lowest` |
| 界面语言 | 简体中文 | 见上 |
| 桌面 / 开始菜单快捷方式 | 默认创建 | |
| 开机自启（热键宿主） | **默认不创建** | 会常驻一个后台进程，应由用户显式选择 |
| 接管文件夹 | **默认不接管** | 会改变「双击文件夹」的系统行为，应由用户显式选择 |
| 安装 / 升级前 | 结束正在运行的实例 | 热键宿主是无窗口进程，Windows 重启管理器看不见它，不主动结束会让覆盖 exe 失败在「文件被占用」 |
| 卸载 | 先还原文件夹接管，再删文件 | 顺序反了会留下指向已删文件的 shell 关联 |
| 用户偏好 `%APPDATA%\pagefold` | **默认保留**，卸载结束时询问 | 静默卸载会按默认按钮自动回答，所以默认项必须是「否」 |

安装包并没有绕开原先那套机制，只是把「装文件 + 建快捷方式 + 调命令」串了起来：
`--install-folder-integration` / `--restore-folder-integration` 仍是唯一的还原通道
（见「文件夹接管的设计与安全性质」）。

## 依赖获取

`.cargo/config.toml` 将 `crates-io` 替换为 USTC 稀疏索引与下载源，规避直连 crates.io 的网络抖动。

> ⚠️ 该文件**禁止**写入任何 `linker` / `ar` 配置。

### `windows` crate 的 feature 必须显式声明

`Win32_*` 里不少条目被别的 feature 门控，容易踩：

- `WNDCLASSW` / `RegisterClassW` 需要 `Win32_Graphics_Gdi` —— 因为 `WNDCLASSW` 的
  `hbrBackground` 字段类型 `HBRUSH` 来自 Gdi 模块；
- `RegCreateKeyExW` / `CreateMutexW` 需要 `Win32_Security` —— 签名里有 `SECURITY_ATTRIBUTES`。

这两个 feature 原本是靠 `gpui-pre-windows` 的 feature 合并「蹭」来的，症状很有迷惑性：
`cargo build --workspace` 一切正常，而 `cargo check -p fm-platform` 直接编译失败。
现在 `fm-platform` 显式声明自己用到的每一个 feature，不再依赖别人的合并结果。


## 版本钉定与升级约束

- `gpui-pre` = `0.3.5`、`gpui-pre-platform` = `0.3.5`、`gpui-component` = `0.6.4`、`gpui-kit-assets` = `0.6.4`，
  全部使用 `=` 精确钉版。
- crates.io 上 `gpui` 官方包停在 0.2.x，0.3.x 由 `gpui-pre-*` 家族提供。上游包自身已完成
  `gpui = { package = "gpui-pre" }` 形式的改名，因此本项目只需做同样的改名，**不需要任何 `[patch.crates-io]` 包重映射**。
- 查询 crates.io 索引时必须同时看 `name`（依赖键名）与 `package`（真实包名）两个字段，只看 `name` 会误判为「包不存在」。
- 升级 GPUI 版本时需重新评估的两件事：
  1. `gpui-pre-windows` 的着色器编译路径是否仍能自动定位 `fxc.exe`（见下节）；
  2. `gpui-pre` 的 default feature 集合是否发生变化（当前为 `font-kit` / `wayland` / `x11` / `windows-manifest`）。

## Phase 0' 决策记录（原生 Windows 构建链路重验证）

早期规划记录过「Linux 交叉编译到 `x86_64-pc-windows-gnu`」的路线，该路线已废弃。
本节记录改用 **Windows 原生 MSVC 构建** 后的重新验证结论。

### 决策 1：不创建 `vendor/` 补丁

早期规划的原方案要求 vendor `gpui-pre-windows-0.3.5` 并对 `directx_renderer.rs` 打补丁，用于绕开两个缺陷。
经核实，**这两个缺陷的成因都是「构建脚本运行在 Linux 上」，在原生 Windows 下均不成立**：

| 原缺陷                                                     | 成因                                                                                                                          | 原生 Windows 下的实际情况                        |
| ---------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| release 交叉编译失败：`couldn't read .../shaders_bytes.rs` | `build.rs` 的 `#[cfg(target_os = "windows")]` 判断的是构建机平台，Linux 上不进入 `compile_shaders()`，文件永不生成            | 构建机即 Windows，该分支恒为真，文件正常生成     |
| debug 产物在 Windows 上无法运行                            | `directx_renderer.rs` 把构建机的 `CARGO_MANIFEST_DIR` 绝对路径烘焙进二进制，Linux 路径在 Windows 上 `canonicalize()` 必然失败 | `CARGO_MANIFEST_DIR` 本身就是合法的 Windows 路径 |

**并且 `gpui-pre-windows 0.3.5` 的 `build.rs` 自带 `find_fxc_compiler()`，按以下顺序查找编译器，无需人工注入 PATH：**

1. 环境变量 `GPUI_FXC_PATH`
2. `where.exe fxc.exe`（PATH）
3. 注册表 `HKLM\SOFTWARE\WOW6432Node\Microsoft\Microsoft SDKs\Windows\v10.0` 的 `InstallationFolder`
   → 取 `bin` 下版本号最大的目录 → 按架构取 `x64/fxc.exe`

本机实测：`GPUI_FXC_PATH` 未设置、PATH 中无 `fxc.exe`，但该注册表键存在且值为
`C:\Program Files (x86)\Windows Kits\10\`，其下最高版本 `10.0.26100.0/x64/fxc.exe` 存在 —— 因此第 3 条回退路径可用。

**结论：`vendor/` 目录不创建，`[patch.crates-io]` 不添加。** 这同时省掉了原方案的三项代价：
运行期编译着色器、`%TEMP%` 落盘副作用、以及「升级 GPUI 必须重打补丁」的长期维护债务。

### 决策 2：debug 与 release 的着色器差异

- **debug**：不编译着色器，运行期直接从 cargo registry cache 中该 crate 的 `src/shaders.hlsl` 加载。
  因此 **debug 产物依赖 registry cache 不被清空**；清缓存后必须重新构建。最终验收使用 release。
- **release**：构建期由 `fxc.exe` 把 HLSL 编译成字节码并内嵌，产物自包含。

### 决策 3：已废弃、不得照搬的配置

以下来自早期规划（交叉编译方案）的内容已全部失效，**照抄会直接导致构建失败**：

- `.cargo/config.toml` 中的 `[target.x86_64-pc-windows-gnu] linker = "x86_64-w64-mingw32-gcc"` 与 `ar`
- 构建命令中的 `--target x86_64-pc-windows-gnu`
- MinGW-w64 工具链（`mingw64-gcc` / `mingw64-binutils` / `mingw64-crt` / `mingw64-winpthreads-static`）
- `rustup target add x86_64-pc-windows-gnu`、`dnf --enablerepo=powertools`

### 验证结果

| 检查项             | 结果                                                                                                                                                   |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 依赖解析           | 828 个包，经 USTC 镜像全部拉取成功                                                                                                                     |
| debug 构建         | 通过                                                                                                                                                   |
| debug 本地运行     | 通过：窗口存活满 15 秒；日志显示`Using GPU: Intel(R) Graphics`、`Created device with Direct3D 11.1 feature level`、`Use Microsoft YaHei UI as UI font` |
| 单元测试           | `cargo test --workspace` 252 项全部通过（1 项标记 ignored），零警告                                                                                     |
| release 构建       | 通过：`Finished release profile [optimized] target(s) in 4m 54s`，0 错误，产物 `target/release/pagefold.exe`（17.8 MB）                              |
| release 产物子系统 | 通过：`PE32+ executable for MS Windows 6.00 (GUI), x86-64`，不弹控制台                                                                                 |
| release 本地运行   | 通过：进程存活满 10 秒；构建期由`fxc.exe` 编译的内嵌着色器字节码可用                                                                                   |
| 外壳与双视图       | 通过：四区块 + 面包屑 + 虚拟化双视图运行满 25 秒，无运行期错误                                                                                         |
| 万级目录扫描       | 通过：**20,400 条目 / 44 ms**（release）；同一目录在 debug 未优化构建下为 121 ms；均无 panic                                                           |

大目录验证方法（20,000 个文件 + 400 个目录）：

```bash
BENCH=target/bench-dir
mkdir -p $BENCH
for i in $(seq 1 20000); do : > "$BENCH/报告_$i.txt"; done
for i in $(seq 1 400); do mkdir -p "$BENCH/子目录_$i"; done
RUST_LOG=info cargo run --release -- "$(pwd)/$BENCH"
# 期望日志：`目录扫描完成 entries=20400 elapsed_ms=<两位数>`
```

注意上述数字只覆盖**扫描与排序**这一侧。滚动帧率依赖 GPU 与窗口尺寸，无法用脚本断言，
需要人眼在真机上看一遍——这是本仓库唯一无法自动化的验收项。

## 已知无害告警

- `DXGI_ERROR_SDK_COMPONENT_MISSING (0x887A002D)` + `Failed to get DXGI debug interface`
  —— 系统缺少 DXGI 调试层（Windows 可选功能「图形工具」），仅在 debug 构建下出现，
  **只影响 DirectX 调试特性，不影响渲染**。需要时可通过「设置 → 系统 → 可选功能 → 图形工具」安装。
- `unable to get cursor position: 拒绝访问 (0x80070005)`
  —— 从非交互式终端（如远程会话）启动时，进程无法读取系统光标位置。
  **不影响渲染与交互**，从资源管理器双击启动时不会出现。

## 图标资产：必须用 `AllAssets`

`gpui_kit_assets` 提供两个资源来源：

- `Assets` —— 只嵌入 `default-icons.txt` 里的**默认子集**；
- `AllAssets` —— 嵌入完整 Lucide 目录（约 1800 个 svg）。

工具栏用到的 `X` / `RefreshCw` / `LayoutGrid` / `List` / `Monitor` / `Download` 等
**不在**默认子集里。若用 `Assets`，运行期会刷出大量
`could not find asset at path "icons/x.svg"`，图标显示为空白。
本项目因此注册 `AllAssets`，代价是二进制体积增加。

## 目录结构

```text
file-manager/
├── docs/                # 设计、开发与路线图文档
├── installer/           # Inno Setup 安装包脚本
├── assets/app-icon/     # 应用图标交付物（75 个文件，见该目录 README）
├── tools/icon-gen/      # 图标生成器：设计参数 → SVG / PNG / ICO / ICNS（独立 workspace）
├── Cargo.toml           # workspace 根：成员、共同依赖、release profile
├── rust-toolchain.toml  # 钉 stable 通道
├── .cargo/config.toml   # USTC 镜像（禁止写入 linker/ar）
└── crates/
    ├── app/       # 入口、窗口引导（自绘标题栏选项）、键位绑定
    ├── shell/     # 标题栏、标签条、命令条、侧边栏、状态栏、响应式布局与内容区
    ├── views/     # 设计令牌、通用控件、面包屑、图标网格、详情表格、空状态、预览面板、驱动器总览
    ├── core/      # Entry / TabState / Selection / Command / Density / IconZoom（不依赖 gpui 与 Windows API）
    ├── services/  # 目录流式扫描、图标缓存、配置、会话持久化、目录变更监听编排
    └── platform/  # FileSystem trait 及 windows / mock 实现（含目录监听、命名管道、热键宿主）
```

界面与图标的关键字：

| 想改什么                     | 改哪里                                                                 |
| ---------------------------- | ---------------------------------------------------------------------- |
| 颜色、字号、圆角、间距、断点 | `crates/views/src/tokens.rs`（唯一来源）                               |
| 区块尺寸与响应式判定         | `crates/shell/src/layout.rs` 的 `Viewport`                             |
| 某个控件的形态               | `crates/views/src/widgets.rs`（图标按钮 / 分段控件 / 勾选框 / 容量条） |
| 主题与组件库色值同步         | `crates/shell/src/shell.rs` 的 `apply_component_theme`                 |
| 设置页的内容与分组           | `crates/views/src/settings_page.rs`（纯展示）+ `crates/shell/src/layout.rs` 的 `render_settings`（状态绑定） |
| 会话恢复（标签 / 历史）      | `crates/services/src/session.rs` + `crates/core/src/tab.rs` 的 `TabSnapshot` |
| 列宽偏好与钳制规则           | `crates/core/src/appearance.rs` 的 `ColumnWidths`；渲染侧在 `crates/views/src/details_table.rs` |
| 目录变更自动刷新的节奏       | `crates/services/src/watch.rs`（换目录/去抖基准）+ `crates/shell/src/shell.rs` 的 `ambient_tick` 与 `TICK_MS`（400ms） |
| 重复启动的位置转发           | `crates/platform/src/windows/ipc.rs`（管道名 `Pagefold.Open.v1`）+ `crates/app/src/bootstrap.rs` 的副本分支 |
| 隐藏/系统文件的判定          | `crates/platform/src/windows/mod.rs` 的 `is_hidden`；过滤点在 `ShellView::begin_load`（按 `config.show_hidden`） |
| 系统图标的提取与渲染         | 提取：`crates/platform/src/windows/icon.rs`；缓存：`crates/services/src/icon.rs` 的 `IconCache`；渲染：`crates/views/src/icon_image.rs` + `widgets::glyph` |
| 撤销栈与逆操作               | `crates/core/src/undo.rs`（记录什么）+ `crates/services/src/ops.rs` 的 `undo`（怎么返回）+ `ShellView::undo_last`（入口与提示） |
| 永久删除                     | `crates/platform/src/windows/fs_ops.rs` 的 `delete_permanently`（含只读清属性）+ `ShellView::delete_selection_forever`（独立确认框） |
| 批量重命名的命名规则         | `crates/core/src/naming.rs` 的 `BatchSpec` / `batch_names`（纯函数）；面板在 `layout.rs::render_batch_overlay`，状态在 `ShellView::open_batch_rename` |
| 传输进度与取消               | 平台：`platform/src/fs.rs` 的 `CancelToken` / `TransferProgress`（含 `bytes_done` / `bytes_total`）+ `fs_ops::transfer_with`（1MB 分块、`copy_mtime`）；服务：`ops::paste_with`；UI：`ShellView::paste`（线程 + 120ms 轮询）与 `layout.rs::render_transfer_card`（进度条 + 百分比） |
| 网络共享（UNC）              | 平台：`platform/src/windows/network.rs`（`is_network_root` / `host_of` / `shares` 走 `NetShareEnum` / `connect_interactive` 走 `WNetAddConnection2W`）；`windows/mod.rs::scan` 分流；外壳：`commit_address_edit` 接受主机根并区分网络错误文案 |
| 热键宿主                     | `platform/src/windows/hotkey_host.rs`：钩子回调只判 `Win+E`，所有耗时动作经 `spawn_off_thread`；窗口过程里只有托盘右键菜单留在消息线程 |
| 跨程序文件剪贴板             | `crates/platform/src/windows/clipboard.rs`（`CF_HDROP` 编解码 + `Preferred DropEffect`）；`ShellView::clip_selection` 负责写、`paste` 负责回退读、`probe_system_clipboard` 负责可用态 |
| 拖入（外部文件拖进来）       | `crates/shell/src/shell.rs` 的 `wrap_list_area`（`on_drop` / `can_drop` / `on_drag_move` / `on_file_drop_exit`）+ `accept_drop` + `begin_transfer`；不需要改平台层 |
| 双击 / 回车打开              | 平台：`fs_ops::shell_open`（`ShellExecuteW`，错误码经 `describe_shell_code` 翻译）；外壳：`ShellView::open_entry` / `open_path` / `shell_open` |
| 回收站还原 / 清空            | `crates/platform/src/windows/recycle_bin.rs` 的 `restore`（读 `$I` → `MoveFile` → 删 `$I`）与 `empty`（`SHEmptyRecycleBinW`）；外壳：`restore_selection` / `empty_recycle_bin` / `start_empty_bin` |
| 缩略图                       | 平台：`platform/src/windows/thumbnail.rs`（`IShellItemImageFactory` + `GetDIBits`）；服务：`services/src/thumbnail.rs`（单线程队列 + 白名单）；外壳：`ShellView::pump_thumbnails` / `patch_thumbnails`（`ambient_tick` 驱动） |
| 预览面板的正文               | 服务：`services/src/preview.rs`（按内容判断文本、32KB / 200 行上限）；视图：`PreviewBody` + `preview_panel::single`（只读 `Textarea` 承载，可框选复制）；外壳：`ShellView::pump_preview_text`（后台读取，按路径防重）+ `sync_preview_input`（实体同步，在 `render` 里做，因为需要 `window`） |
| 递归搜索                     | 服务：`services/src/search.rs`（深度优先遍历 + 批量回传 + `CancelToken`）；外壳：`start_search` / `pump_search` / `toggle_recursive_search`，结果由 `visible()` 优先返回 |
| 文件夹接管图形面板           | 平台：`folder_association::{state, set_context_menu, set_default_handler, restore_all}`（托盘菜单与设置页共用同一组）；视图：`settings_page::integration_card` + `FolderIntegration`；外壳：`folder_integration` 缓存 + `toggle_folder_menu` / `toggle_folder_default` / `restore_folder_integration` |
| 快捷键自定义                 | 领域：`core/src/shortcut.rs`（`KeyCombo` 解析/规范化、`COMMANDS` 目录、`resolve`、`conflicts`、`occupant`）；服务：`Config.shortcuts` / `Config.global_hotkey`；外壳：`shell/src/shortcuts.rs`（配置 → `KeyBinding` 表，含 id→Action 的 match）+ `shell/src/shortcut_handlers.rs`（录制浮层、改键、启停、恢复默认、同步宿主）；视图：`settings_page::shortcuts_card` |
| 全局热键（系统注册）         | 平台：`platform/src/windows/hotkey.rs`（`mods_and_vk` / `register` / `probe` / `hotkey.txt` 读写 / `WM_HOST_RELOAD`）；宿主：`hotkey_host.rs` 的 `register_configured_hotkey` + `WM_HOTKEY` 分支 |
| 应用图标的几何与配色         | `tools/icon-gen/src/design.rs`，改完重新生成（见 `assets/app-icon/README.md`） |

用户数据的两个落点（都在 `%APPDATA%\pagefold\`，损坏时都只告警并回退默认）：

| 文件 | 内容 | 归属 |
| --- | --- | --- |
| `config.json` | 主题、视图模式、密度、缩放、扩展名、侧边栏折叠、预览开关、`detail_columns` | `fm_services::config` |
| `session.json` | 标签顺序、各自历史与游标、视图模式、激活下标、格式版本 | `fm_services::session` |

分层约束：UI 层完全不感知平台差异，所有原生能力经 `platform` 层注入；
`core` 只描述数据与状态流转，不依赖 gpui、Windows API 或文件系统实现。
