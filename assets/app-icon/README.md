# 叠页 · 应用图标（设计稿 C 方案）

本目录的全部位图与容器文件都由 `tools/icon-gen` **从矢量源生成**，不要手工编辑产物：
改设计参数 → 重新生成，是唯一允许的修改路径。

```bash
# 重新生成（约 2 秒；工具不属于产品 workspace，需要单独指定清单）
cargo run --release --manifest-path tools/icon-gen/Cargo.toml
```

- 矢量源（`src/*.svg`）由生成器写出，与设计稿逐属性一致：同样的路径数据、同样的组变换
  `translate(512 512) scale(1.08) translate(-512 -512)`、同样的 `stroke-width="20"`。
- 每个尺寸都是**在目标分辨率下重新栅格化**，不是把 1024 缩小——小尺寸不会先糊再缩。
- 生成结束会跑 14 项自检（安全区、层叠缝隙、镂空遮罩、iOS 无 alpha、容器结构、清单完整性…），
  任一项失败即以非零码退出，产物不允许交付。

---

## 1. 设计依据：C 方案「层叠文件夹」

取自 `design/app-icons.html` 的候选 C。设计稿对它的判断是：
**类别识别最快、16px 下轮廓最稳**，代价是与同类产品撞车、记忆点最弱——
因此它是一个「稳妥」的选择：用户一眼就认出这是文件工具。

### 几何（全部写在 1024 画布上）

| 元素 | 取值 | 来源 |
|---|---|---|
| 底板 | 超级椭圆 `n = 5`，半宽 412（画布 100..924），等效圆角 22.1% | 设计稿 `#plate` |
| 后层文件夹 | 路径起于 `(346, 302)`，圆角 44 | 设计稿 `#g-c` |
| 前层文件夹 | 路径起于 `(410, 366)`，圆角 44 | 设计稿 `#g-c` |
| 标记缩放 | `1.08`（绕画布中心） | 设计稿组变换 |
| 缝隙（标记描边） | `20` 组内单位 = **21.6** 画布单位 | 设计稿 `stroke-width` |
| 安全区 | 标志宽度 ≤ 画布的 **55%** | 设计稿规格表 |

「层叠」这件事全靠那条**底板色缝隙**：前层文件夹用底板色描边，压在后面的文件夹上时
就切出一道缝。默认参数下标记占宽 **46.4%**，安全区内。

### 配色

| 形态 | 底板 | 标记 | 描边 | 说明 |
|---|---|---|---|---|
| 实底（默认） | `#076be3` = `oklch(55% 0.2 258)` | `#ffffff` | 与底板同色 | 对比最强，小尺寸最立得住 |
| 浅底 | `#e4efff` | `#076be3` | 底板 `#c4d9f9` / 缝隙 `#e4efff` | 贴合浅色界面、需要低对比度时 |
| 深底 | `#0f1f37` = `oklch(24% 0.05 258)` | `#ffffff` | 与底板同色 | 深色桌面 / 深色模式 |
| 透明背景 | 无 | `#076be3` 或 `#ffffff` | 缝隙**镂空** | 需要把标记叠在自有底色上时 |

深色底板与软色不在设计稿的位图里，而是以 OKLCH 写在其 CSS 变量中，
生成器用同一条 `OKLCH → sRGB` 换算链算出来（`#0f1f37` 就是这么来的，不是查色卡抄的）。
设计稿的软色令牌（`--ic-tint-soft` / `--ic-deep-soft`）没有出现在本目录：
它们专门服务 B 方案那两枚页签，C 方案里没有对应元素。

---

## 2. 命名规则

| 位置 | 命名 | 含义 |
|---|---|---|
| `src/` | `icon-c-folder.svg`<br>`icon-c-folder-<变体>.svg` | 矢量源；`变体` ∈ `tinted` / `deep` / `glyph` / `glyph-white` / `square` / `circle` |
| `png/` | `icon-c-folder-<像素>.png` | 实底主图集，边长即文件名 |
| `png/` 之外 | `icon-c-folder-<变体>-<像素>.png` | 变体位图（见 `variants/`） |
| `windows/` | `icon.ico`、`icon.rc` | Windows 图标容器与资源脚本 |
| `macos/` | `icon.icns` | macOS 图标容器 |
| `android/` | `mipmap-<密度>/ic_launcher*.png` | 直接对应 Android 资源目录 |
| `ios/` | `AppIcon-<尺寸>@<倍率>x.png` | 与 `Contents.json` 中的条目一一对应 |
| `web/` | `<用途>-<像素>.png` | 用途优先，便于在 HTML 里直接引用 |
| `variants/` | `<模式>/icon-c-folder-<变体>-<像素>.png` | 模式 ∈ `dark` / `light` / `transparent` |

约定：**像素写在文件名里**，任何尺寸都可以只看名字确认，不需要打开图片。

---

## 3. 文件清单与用途

| 路径 | 数量 | 用途 |
|---|---|---|
| `src/*.svg` | 7 | 矢量源（可缩放、可编辑）；Web 直用与大尺寸印刷的兜底 |
| `png/` | 9 | 16 / 24 / 32 / 48 / 64 / 128 / 256 / 512 / 1024 的通用位图 |
| `windows/icon.ico` | 1 | Windows 任务栏、资源管理器、快捷方式；内含 16/24/32/48/64/128/256 |
| `windows/icon.rc` | 1 | 资源脚本（**已接入 `fm-app` 构建**，见 §6） |
| `macos/icon.icns` | 1 | macOS Dock / 访达 / 启动台；内含 16…1024 共 11 个槽位 |
| `android/mipmap-*/` | 15 | 传统启动图、圆形启动图、自适应前景层（48…192 / 108…432） |
| `android/values/` | 1 | 自适应图标背景色（`ic_launcher_background`） |
| `android/mipmap-anydpi-v26/` | 2 | 自适应图标声明（API 26+） |
| `ios/` | 16 | 15 张 AppIcon（20…1024）+ `Contents.json` |
| `web/` | 10 | favicon、Apple Touch Icon、PWA 192/512、maskable、主题自适应 SVG、manifest |
| `variants/dark/` | 3 | 深色模式：256 / 512 / 1024 |
| `variants/light/` | 3 | 浅色 / 低对比：256 / 512 / 1024 |
| `variants/transparent/` | 6 | 透明背景：蓝标与白标各 256 / 512 / 1024 |

合计 75 个文件、670 KB（其中 `macos/icon.icns` 占 118 KB——它内部有 11 张位图）。

---

## 4. 平台适配要点

### Windows（`.ico`）
- 7 个尺寸：16 / 24 / 32 / 48 / 64 / 128 / 256。
- **≤64px 用 BMP**（32 位 BGRA + AND 掩码）、**≥128px 用 PNG**：
  老程序只认 BMP，新外壳读 PNG 更省体积。BMP 的高度字段按 ICO 约定写成实际高度的两倍。
- `.ico` 里的小尺寸用「加固版」（见 §5），大尺寸保持设计稿取值。

### macOS（`.icns`）
- 11 个数据块：`icp4`(16) `icp5`(32) `icp6`(64) `ic07`(128) `ic08`(256) `ic09`(512) `ic10`(1024)
  以及 `@2x` 槽位 `ic11`(32) `ic12`(64) `ic13`(256) `ic14`(512)，全部为 PNG 载荷。
- 放进 `.app/Contents/Resources/` 并在 `Info.plist` 里指向 `CFBundleIconFile` 即可。

### Android
- 传统：`mipmap-{mdpi,hdpi,xhdpi,xxhdpi,xxxhdpi}/ic_launcher.png` = 48 / 72 / 96 / 144 / 192。
- 圆形启动器：`ic_launcher_round.png`（满幅圆形底板，同尺寸）。
- 自适应（API 26+）：`ic_launcher_foreground.png` 是 108dp 画布（108…432），
  标记压到画布 **42%**——系统只保证中央 66dp 圆可见，而标记包围盒是正方形，
  对角线是边长的 √2 倍，因此上限是 `61% / 1.414 ≈ 43%`；背景走颜色资源
  `@color/ic_launcher_background`，避免视差缩放时把位图放大出锯齿。
- 直接把 `android/` 里三个目录（`mipmap-*`、`values`）合并进 `res/` 即可。

### iOS
- 15 张 AppIcon（20/29/40/60/76/83.5 的 1x–3x + 1024 营销图），
  **全部是 RGB PNG（color type 2，不含 alpha 通道）**——App Store 校验会拒绝带透明度的图标，
  只把 alpha 写成 255 还不够，颜色类型本身必须是 RGB。
- 满幅直角方形、不加圆角：圆角由系统裁切。标记占宽 52%。
- `ios/Contents.json` 可直接放进 `Assets.xcassets/AppIcon.appiconset/`；
  其中 20@2x 同时属于 iPhone 与 iPad，两条目指向同一文件。

### Web
- `favicon.ico`（16/32/48）+ `favicon-16/32/48.png`。
- `apple-touch-icon-180.png`：满幅方形、无 alpha（iOS Safari 书签图标同样不接受透明）。
- PWA：`icon-192.png` / `icon-512.png` / `icon-maskable-512.png`，并在 `site.webmanifest` 中声明。
  maskable 版本把标记压到 42%，落在 80% 直径的安全圆内。
- `web/icon.svg` 内嵌 `prefers-color-scheme`：**同一个文件**在深色模式下自动换成深色底板
  `#0f1f37`，浅色模式用 `#076be3`。这是唯一一份"自适应深浅色"的产物。

---

## 5. 深浅色与小尺寸的处理

### 深浅色
| 场景 | 用哪个 |
|---|---|
| 默认（Windows 任务栏、桌面、开始菜单） | `png/` 或 `windows/icon.ico`（实底） |
| 深色桌面 / 深色主题 | `variants/dark/`（深底 `#0f1f37`） |
| 浅色界面内嵌、需要更克制 | `variants/light/`（浅底） |
| 叠在自有底色 / 需要单色 | `variants/transparent/`（蓝标或白标，缝隙镂空） |
| 网页（自动跟随系统） | `web/icon.svg` |

### 小尺寸加固（≤32px）
设计稿的尺寸阶梯是**同一份几何的等比缩放**，而 21.6 画布单位的缝隙在 16px 下只有
**0.34 个物理像素**——它会退化成一条若有若无的灰线，两层文件夹的分隔随之消失。
因此 16 / 24 / 32 三个尺寸单独做一次加固：

- 描边加粗到 **0.9 物理像素**（对应画布单位：16px → 57.6、24px → 38.4、32px → 28.8）；
- 标记放大 **1.08 倍**；
- 加固后仍强制满足设计稿的 55% 安全区（超出即夹回，`Spec::clamp_mark_scale`）。

≥48px 一律保持设计稿取值（描边 21.6 画布单位、缩放 1.08），不做任何调整。

实测：16px 下标记占 **23 个像素**、底板占 117 个像素（共 256）——形状清晰可辨；
加固后 16px 的实际描边宽度是 0.90px，256px 仍是设计稿的 21.6 画布单位。

> 换句话说：**「与设计稿完全一致」在小尺寸上是「加固后的等价形态」**，
> 而不是逐像素复刻设计稿的 16px 等比缩略——两者在这里是矛盾的，
> 本目录选择了「设计稿要 16px 站得住」这条更上位的要求。这一条是唯一的刻意偏离，
> 且只发生在 ≤32px 的三个尺寸上。

---

## 6. 重新生成与接入

### 重新生成
```bash
cargo run --release --manifest-path tools/icon-gen/Cargo.toml   # 生成 + 14 项自检
cargo test  --release --manifest-path tools/icon-gen/Cargo.toml # 16 项单元测试
```
生成器**不属于产品 workspace**（`members = ["crates/*"]`），因此
`cargo build --workspace` 不会碰它，也不需要往仓库根 `Cargo.toml` 写 `exclude`。

改了设计稿之后要动的地方只有一处：`tools/icon-gen/src/design.rs` 顶部的几何常量与色值。

### 自检清单（14 项）
四角透明、中心为白、**层叠缝隙存在且为底板色**、透明版缝隙确实镂空、
满幅方形不透明 / 圆形四角透明、**全部 300 个「规格 × 尺寸」组合都在 55% 安全区内**、
16px 标记面积可辨识、小尺寸加固生效、iOS 全部为 RGB 无 alpha、
ICO 目录自洽、ICNS 数据块自洽、主图集 PNG 头与文件名一致、
深浅底色值来自设计稿令牌、交付清单完整。

### 接入状态

- ✅ **exe 图标已嵌入（Windows）**：`crates/app/build.rs` 在构建期用 `embed-resource`
  （已是 `gpui-pre` 的 build-dependency，故未新增依赖）编译 `windows/icon.rc`，
  产物经 `cargo:rustc-link-arg-bins` 交给链接器。图标因此烘焙进 PE 资源段，
  **产物仍是单文件，运行期没有新增依赖**；已核实 `.ico` 的 7 个尺寸全部出现在
  debug 与 release 的 `files-gpui.exe` 中。

  两处容易被踩到的地方，改动时请留意：
  1. `rc.exe` 解析相对路径的基准是**调用者的工作目录**（Cargo 跑 build.rs 时是 crate 根），
     不是 `.rc` 所在目录 —— 所以 build.rs 必须把资源目录显式加进 `/I`。
  2. 资源 ID 用数值 `1`。`IDI_APPICON` 这类符号名**不是** rc.exe 的预定义符号
     （SDK 里只有 `IDI_APPLICATION` / `IDI_ERROR` 等），未定义标识符会被当成字符串名资源；
     命名项在 PE 资源目录里排在数值项之前，目前只有一个图标组所以照样生效，
     但那是「以后多出一个图标组才炸」的隐患。

  降级与失败边界：找不到 `rc.exe`（未装 Windows SDK）只输出一条 warning 并跳过嵌入，
  构建照常完成、exe 显示系统默认图标；资源脚本本身有语法错误则中断构建。
  注意 Cargo 会缓存构建脚本的输出，**先在不带 SDK 的机器上构建失败/跳过、
  再补装 SDK** 时需要 `cargo clean -p fm-app`（或改动资源文件）才会重新嵌入。
- ⚠️ **macOS 未打包**：`icon.icns` 已就绪，但仓库目前没有 `.app` 打包流程。
- 托盘图标仍使用系统默认图标（`IDI_APPLICATION`），后续可换成 `icon.ico`。
