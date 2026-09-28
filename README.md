# Files（GPUI 版）

面向 Windows 的现代桌面文件资源管理器，中文名「文件夹管理器」，
界面按 `design/` 下的设计稿实现（设计稿里用的代号是「叠页」，指同一件东西），
Fluent Design / Windows 11 原生观感，参考 Files 项目。
需求基线见 [`.codebuddy/plans/PLAN.md`](./.codebuddy/plans/PLAN.md)。

---

## 界面实现说明（依据 `design/`）

设计稿是三个 HTML 原型：`file-manager.html`（主界面）、`file-manager-settings.html`（设置页，
本次未实现）、`app-icons.html`（应用图标四稿）。主界面按设计稿逐条落地，取值不再散落在各处：

- **设计令牌唯一来源**：`crates/views/src/tokens.rs`。设计稿用 OKLCH 表达颜色，
  这里实现了一整条 `OKLCH → OKLab → 线性 sRGB → HSL` 转换链，并适配 GPUI 的
  `Hsla.h` 取值是 **0..1**（不是 0..360）这一单位差异——
  单元测试直接断言 `oklch(55% 0.2 258) ≈ #076be3`，与设计稿公布的换算值一致。
  派生色（`accent-soft` 16%、`accent-line` 45%、`hover` 6%…）按设计稿语义取「原色 + 透明度」。
- **尺寸与响应式**：字号 11/12/13/15/17、圆角 6/10、控件 30px、
  标题栏 38 / 标签条 38 / 标签 32 / 地址栏 32 / 列表头 28 / 状态栏 26、
  侧边栏 238（折叠 48）、预览面板 306 全部集中在 `tokens::metrics`；
  断点 1240 / 1100 / 980 / 920 / 720 / 560 集中在 `tokens::breakpoints`，
  由 `fm-shell::layout::Viewport` 一次性判定「当前宽度下哪些控件存在」。
- **组件库主题同步**：地址栏与搜索框用的是 `gpui_component::Input`，
  它的描边 / 底色 / 圆角读组件库主题，因此 `fm-shell::shell::apply_component_theme`
  会把设计令牌里的关键色值同步过去，避免这一个控件与周围割裂。

## 已实现能力

- **主窗口四区块**：标签条、命令条、内容区（侧边栏 / 主区 / 预览面板三栏）、底部状态栏。
  窗口以 `appears_transparent: true` 创建，系统边框隐藏；主题切换与最小化 / 最大化 / 关闭
  都在标签条右端，动作交回系统（`WindowControlArea`），不自己实现窗口缩放逻辑。
  标签与这组按钮之间那段空白是**窗口唯一的拖动区域**——原来承担拖拽的品牌栏已整条删除，
  所以那一段留了 32px 最小宽度，保证标签铺满时窗口仍抓得住。
- **多标签页**：新建 / 关闭 / 切换，每个标签独立维护路径、前进后退历史、视图状态与扫描代数。
  关闭当前标签时优先落到右侧邻居，关闭最后一个标签时重置为新标签。
- **左侧导航栏**：快速访问（由 `SHGetKnownFolderPath` 定位，正确处理被重定向过的文件夹）、
  **回收站**（虚拟位置，只读浏览，见下）、
  「此电脑」分组（可折叠，子项为驱动器）、底部磁盘容量档位（名称 + 百分比 + 容量条 + 剩余/总量）。
  选中项为强调色浅底 + 强调色图标；展开 238px、折叠 48px，切换带 180ms 缓动。
- **地址栏**：面包屑逐级可点击跳转，层数过多时保留首段与尾部、中间折叠为省略号；
  窄屏（≤720px）只保留首段与末段。点击空白处切换为可编辑地址栏，
  **回车提交、失焦取消**；右侧「复制路径」写入系统剪贴板（`ClipboardItem`）并弹出通知。
- **搜索框**：点击进入编辑态，**输入即筛选**当前文件夹（大小写不敏感的名称匹配），
  无结果时给出专门的空状态；清除按钮一键回到完整列表。
- **双视图**：图标网格（按行虚拟化，单元格 122/152/192 三档，1:1 缩略图底板 + 两行名称）
  与详细信息表格（固定表头 + 按行虚拟化，列宽按断点退让，点击列名排序并显示箭头）。
  两个视图都支持**双击文件夹进入**（文件的双击暂不处理，见「已知缺口」）。
- **选中与预览**：单击选中、`Ctrl` 切换、`Shift` 区间、表头勾选框全选；
  选中后出现选中条（数量 / 合计大小 / 动作）与右侧预览面板
  （单项显示名称、类型、大小、修改时间、位置；多项显示清单与合计）。
- **右键上下文菜单**：详情行、图标单元格、内容区空白处各有一套。条目菜单按类型动态生成
  （驱动器隐藏整组写操作、多选时文案带数量与「批量」、回收站里只给「属性」、
  空白处菜单在「此电脑」与回收站里不显示新建与粘贴）；
  定位与边缘吸附、`Esc` 与点击外部关闭、上下键切换与回车确认由组件库的 `PopupMenu` 提供，
  没有自己造弹出层。
- **文件写操作**：新建（文件夹 / 文本文档 / Markdown 文档）、复制、剪切、粘贴、重命名、
  移到回收站。入口有三处且**共用同一套动作定义**：内容区右键菜单、选中条的动作组、
  命令条右端的「新建」下拉。重命名是**内联**的（在行内把名称换成输入框、预选主干不含扩展名、
  回车或失焦提交）；**新建之后（文件夹与文件都一样）立即进入重命名**，并会把新条目
  滚进可视区（它的位置由排序决定，可能在视野之外）；
  移到回收站前有一个把确定按钮染成危险色的确认框。操作结果与失败原因都以 toast 提示，
  出现在左下角（与设计稿的 `.toasts` 同侧同距）。
  设计与取舍见「文件写操作的设计」一节；仍未做的部分见「已知缺口」。
- **深浅色主题与外观档位**：命令条一键切换主题；状态栏三档缩略图缩放；
  界面密度（紧凑 / 标准 / 宽松）与「显示扩展名」由配置提供。
  全部持久化到 `%APPDATA%\files-gpui\config.json`。
- **大目录流式扫描**：后台线程扫描与排序，结果按 512 条分批并入视图；
  `generation` 计数保证快速切换目录时旧结果不会错乱，扫描中取消会立即停止 IO。
  列表渲染按视口虚拟化，复杂度与目录条目数无关。
- **单实例**：同一登录会话内只保留一个进程。重复启动时新进程会激活已有窗口后立即退出，
  不会堆积多份实例。实现走命名互斥体 + `SetForegroundWindow`，**不写任何注册表**。
- **「此电脑」驱动器总览**：侧边栏顶部可进入，以卡片形式列出全部驱动器
  （图标 + 类型 + 剩余容量 / 总容量 + 占用条）。虚拟位置由 `Location` 枚举表达，
  从类型上就不可能被当成目录送去扫描。
- **回收站（只读浏览）**：侧边栏「访问位置」的最下面进入。直接读各卷
  `$Recycle.Bin\<SID>\` 里成对的 `$I`（元数据）/ `$R`（内容）——**不走** Shell 命名空间那套 COM，
  因为这里需要的只是「删了什么、多大、什么时候删的」。列表显示**原文件名**、大小与**删除时间**，
  可排序、可搜索；读不到的卷与损坏的 `$I` 一律跳过，不让整个视图报错。
  **不做还原与清空**：那两条是写操作。回收站里的条目也不能按路径导航
  （`path` 指向受保护的 `$Recycle.Bin` 内部），双击、右键「打开」与预览面板的「打开」
  三处都做了拦截。
- **`Win+E` 接管**：由独立的热键宿主进程（`files-gpui.exe --hotkey-host`）用低层键盘钩子
  （`WH_KEYBOARD_LL`）拦截 `Win+E`，激活已有窗口或拉起一个新窗口，
  初始位置为「此电脑」——与资源管理器 `Win+E` 的语义一致。
  宿主不启动 GPUI、不占 GPU，常驻内存只有几 MB；
  **本项本身不写任何注册表**（文件夹接管是另一个独立开关），
  托盘菜单「退出」即彻底交还系统。
- **文件夹接管**：两个彼此独立的开关，入口都在托盘菜单里，带勾选态反映真实状态。
  - 「在文件夹右键菜单中显示『用 Files 打开』」——不改变任何默认行为，零风险；
  - 「设为文件夹默认打开程序」——覆盖 shell 的文件夹打开处理器，桌面双击文件夹、
    以及其他程序调 `ShellExecute`（浏览器的「在文件夹中显示」、下载完成的「打开文件夹」）
    都会打开 Files。

  两项都写在 `HKCU\Software\Classes`（用户级，不需要管理员权限），
  因此**还原只需删掉这些键**——机器级 `HKLM` 里的原始值原封不动，行为自动恢复。
  还原入口有三个，彼此等效：托盘菜单「恢复资源管理器为默认」、
  命令行 `--restore-folder-integration`、以及程序目录旁的 `restore-explorer.bat`
  （最后一个是程序本体损坏时的自救通道，见下）。

### 已知缺口（后续阶段）

界面部分（按设计稿对照）：

- **文件写操作的主体已在本次落地**（见「文件写操作的设计」），剩下这些没做：
  - **批量重命名**：需要一个「名称前缀 + 起始序号 + 实时预览」的对话框
    （设计稿的 `#dlgRename`），菜单里保留位置但禁用；
  - **传输进度面板与取消**（设计稿的 `.transfer`）：大目录复制目前是「界面不卡、
    完成后弹一条汇总」，中间看不到进度，也不能中断；
  - **撤销**：设计稿的重命名 toast 带一个「撤销」按钮，那需要一套撤销栈；
  - **永久删除**（`Shift+Delete`）：目前只有「移到回收站」；
  - **与资源管理器互通的剪贴板**：现在的剪贴板是应用内的（纯内存），
    因此「在本程序里复制、切到资源管理器粘贴」还不成立；
  - **复制符号链接与目录联接**：真实实现会**拒绝并如实报错**，不跟随——
    跟随可能把同一棵树复抄多遍，遇到指向上级目录的联接时还会无限递归。
- **回收站只支持浏览**：侧边栏可进入，能列出被删项目（原文件名 / 大小 / 删除时间）、
  可排序、可搜索；但**还原**与**清空回收站**未实现（都是写操作，与上一批同类）。
  另外回收站里的条目不能按路径导航——它们的 `path` 指向受保护的 `$Recycle.Bin` 内部，
  双击、右键「打开」、预览面板的「打开」三处都做了拦截。
  回收站里的右键菜单因此只有「属性」一项（见 `fm-shell` 的 `recycle_bin_menu_spec`）。
- **设置页未实现**：`design/file-manager-settings.html` 是一个独立页面，
  本次只交付主界面。因此「界面密度」与「显示扩展名」目前只能改配置文件
  （`%APPDATA%\files-gpui\config.json` 的 `density` / `show_ext`），
  界面上没有开关；`ShellView::set_density` 作为预留入口保留。
- **「设置 / 帮助」入口未实现**：设计稿的标题栏右侧除了主题切换还有这两个按钮，
  而标题栏本身已按反馈**整条删除**（主题切换与三个窗口按钮移到了标签条右端），
  两个入口目前无处安放；设置页本身的情况见上一条。
- **标签条的「更多」按钮未实现**（设计稿里有，用于标签页操作菜单）。
- **拖放（drag & drop）未实现**：既不能把条目拖出去（拖到资源管理器 / 其它程序），
  也不能把外部文件拖进来。`platform` 层的 trait 边界已留好
  （见 `crates/platform/src/lib.rs` 的模块说明）。
- **目录变更没有实时监听**：外部程序增删文件后本程序不会自动刷新，需要手动刷新
  （命令条按钮，或内容区空白处右键 →「刷新」），或切走再切回来。
- **键盘快捷键未绑定**：设计稿的 `Ctrl+T` / `Ctrl+W` / `Ctrl+L` / `F2` / `Delete`
  等尚未接入；目前所有操作都需要鼠标。
- **缩略图与文件内容预览未实现**：预览面板展示的是元信息（名称 / 类型 / 大小 / 修改时间 / 位置），
  不读取文件内容，也不生成图片缩略图。
- 预览面板的「在列表中定位」按钮未实现。**滚动句柄已经接好了**（新建与改名之后
  会用它把条目带进视野，见 `ShellView::scroll_into_view`），因此这个按钮现在只是
  「拿到条目下标 → 调一次 `scroll_to_item`」的事，缺的只是按钮本身与它的回调。
- 双击文件不会打开——需要用默认程序打开，走 `ShellExecute`，属 `fm-platform` 的后续工作；
  在那之前刻意不做任何事，避免把文件当目录去扫描。
- **框选（橡皮筋）未实现**：单击 / `Ctrl` 点选 / `Shift` 区间 / 表头全选都已可用，
  但按住左键拖出一块矩形来批量选中还没有。底层选择模型当初就是按区间选择设计的
  （`anchor` 锚点 + 有序集合），扩展时不需要改调用方。
- **搜索只过滤当前目录，不递归**：匹配的是**当前目录已加载条目**的名称
  （大小写不敏感的子串匹配），不会下钻子目录，也没有「搜索整个磁盘」这类索引。
  优点是边扫边筛、大目录下即时出结果；代价是找不到子目录里的东西。
- 列宽拖拽、标签过多时的横向滚动与左右箭头未实现。
- 标签不能拖拽排序，也没有「标签页右键菜单」。
- 图标视图的滚动位置按**行号**锚定：调整窗口宽度改变列数后，「第 N 行」对应的条目整批换掉，
  表现为视图跳位（虚拟化网格的固有取舍）。
- 图标网格的列数按推算宽度计算（GPUI 未暴露子元素实测宽度），
  因此侧边栏 180ms 折叠动画期间可能出现最多一列的瞬时误差。
- 系统文件图标（`SHGetFileInfoW`）未接入，当前按扩展名做粗分类（图片 / 音频 / 视频 / 文本 等）。
- 隐藏文件与系统文件未过滤（`.` 开头的条目会照常列出，与资源管理器的默认行为不同）。
- 单实例只做「激活已有窗口」，**尚未转发命令行路径**：已有实例在运行时，
  用 `files-gpui.exe D:\x` 启动第二个进程不会让已有窗口跳到 `D:\x`。
  需要一条命名管道之类的 IPC 通道，属后续增量。
- **Windows 的 exe 图标已嵌入；macOS / Linux 未打包**：设计稿 C 方案（层叠文件夹）的
  全平台图标集在 [`assets/app-icon/`](./assets/app-icon/README.md)，含 75 个文件
  （SVG 源、16…1024 位图、`.ico`、`.icns`、Android / iOS / Web 适配集、深浅色与透明背景变体），
  由 `tools/icon-gen` 生成并自检。Windows 侧由 `crates/app/build.rs` 在构建期把
  `windows/icon.rc` 交给 `embed-resource` 编译，产物经 `cargo:rustc-link-arg-bins` 给链接器：
  **图标烘焙进 PE 资源段，产物仍是单文件、运行期无新增依赖**。构建机上找不到 `rc.exe`
  （未装 Windows SDK）时只输出一条 warning 并跳过嵌入，不中断构建。
  仍未做的：`icon.icns` 没有 `.app` 打包流程接入。
  托盘图标与任务栏按钮现在读的是**同一份 exe 资源**（资源 ID 1），不再是系统默认图标。
- `Win+E` 接管对**提权窗口**无效：低层键盘钩子在安全桌面上收不到按键。
- 常驻的只有热键宿主，主窗口仍是关闭即退出；因此 `Win+E` 的响应时间取决于主程序冷启动耗时，
  标签页与浏览位置不跨重启保留（会话持久化尚未实现）。
- 文件夹接管的边界（**是 Windows 本身的限制，不是实现缺陷**）：
  - **任务栏的「文件资源管理器」按钮拦不到**。实测该固定项 `File Explorer.lnk`
    的 `TARGET` 为**空**、文件仅 407 字节且**不含任何 GUID**，
    说明它指向的是 shell 命名空间项而非文件夹路径 —— 也就是资源管理器本体，
    根本不经过「文件夹动词」那条路（连「覆盖某个 CLSID 的动词」也不成立，因为查无此 CLSID）。
    右键它弹出的 Jump list（最近文件夹）同理。想换掉它只能替换任务栏固定项本身
    （固定 Files、取消固定资源管理器），那属于个人化设置而非程序能力。
  - 「此电脑」「回收站」走 CLSID 命名空间，同样不受影响；
  - 要连这些一起接管只能改 `Winlogon\Shell` 让本程序充当整个桌面外壳，
    而那需要任务栏、桌面图标层与开始菜单，超出本项目范围。
- 文件夹接管有一个真实足控：覆盖生效后若程序被删除或移动，双击文件夹将打不开，
  只能靠 `regedit` 或 `restore-explorer.bat` 修复。因此**卸载前务必先还原**。
- 已有主窗口时，用 shell 打开另一个文件夹只会**激活**已有窗口，不会让它跳到新路径
  （与单实例的限制同源，都需要一条 IPC 通道）。
- 文件夹接管没有图形化设置面板，控制面是托盘菜单与命令行；程序内开关属后续增量。

---

## 文件写操作的设计

入口有三处，**共用同一套动作定义**（`crates/shell/src/context_menu.rs` 的 `MenuAction`）：
内容区右键菜单、选中条的动作组、命令条右端的「新建」下拉。同一个动作在应用里只有一处定义，
否则措辞与禁用逻辑迟早分叉。

### 分层

| 层 | 负责 |
| --- | --- |
| `fm-core::naming` | 名称校验（非法字符、保留设备名、长度按 UTF-16 码元计）、扩展名切分、重名去重 |
| `fm-platform` | 真正的 IO：新建、改名、复制、移动、移入回收站 |
| `fm-services::clipboard` | 应用内剪贴板：复制 / 剪切的状态，以及「这次能不能粘贴」的判定 |
| `fm-services::ops` | 编排：提交前校验名称、分派动作、把回执翻译成一句人话 |
| `fm-shell` | 后台线程执行 → 弹提示 → 刷新列表；以及内联重命名与确认对话框 |

### 几个刻意的选择

**冲突策略是「自动改名」，且永不静默覆盖。** 新建与粘贴遇到重名会得到
`新建文件夹 (2)` / `报告 (2).docx`（序号插在主干与扩展名**之间**，与资源管理器一致），
而不是覆盖或报错。反过来，**重命名**是用户明确给出的名字，重名就报错——
你输入 `报告.docx` 却得到 `报告 (2).docx` 是更坏的结果。
注意 `fs::rename` 在 Windows 上走 `MOVEFILE_REPLACE_EXISTING`，默认会**静默覆盖**
同名目标，因此改名路径上有一道必需的存在性检查（见 `crates/platform/src/windows/fs_ops.rs`）。

**复制与移动走 `std::fs`，只有删除走 shell。** `SHFileOperation` 能免费拿到系统进度条，
但它只回一个错误码，说不出「已复制 3 项，1 项因占用失败」，而 UI 需要的是逐条回执
（`OpReport`）。真正非 shell 不可的只有「移入回收站」——那不是文件系统操作，
而是 shell 的语义：要在每个卷的 `$Recycle.Bin\<SID>\` 下写一对 `$I`/`$R`。

**长路径。** Rust 的 `std::fs` 不会给路径加 `\\?\` 前缀，因此超过 260 字符会直接失败。
对文件管理器来说这不是边角情况（`node_modules` 里三层就两百多字符），所以平台层的
每一次 IO 都经过统一的转换——含 UNC 改写成 `\\?\UNC\`，以及路径里出现 `.` / `..`
时的保守回退（加前缀后系统不再做规范化，`..` 会被当成一个字面量的目录名）。

**跨卷移动**自动退化为「复制 + 删源」：`rename` 无法跨卷，而用户不该关心两个盘
是不是同一个卷。

**移入回收站用独立线程。** `SHFileOperationW` 在需要显示进度时要有能泵消息的线程，
借 GPUI 的后台线程去跑等于让它的消息泵去服务另一套 UI；因此每次删除起一个短命线程。
标志位是 `FOF_ALLOWUNDO`（核心，没有它就成了永久删除）+ `FOF_NOCONFIRMATION`
（确认框我们自己出）+ `FOF_WANTNUKEWARNING`（超过回收站配额、将被永久删除时**仍然要问**）。
刻意不加 `FOF_SILENT`：宁可让系统显示自己的进度条，也不要让删一个几 GB 的目录
看起来像卡死。返回值为 0 也要再查一次 `fAnyOperationsAborted`——用户可能在系统进度条上
点了取消，那时报「已删除 3 项」就是在说谎。

**新建之后立刻改名，为此不走「等扫描完成」那条路。** 设计稿的 `makeFolder()` 是
「建完立刻进入重命名」，而目录扫描是异步的（大目录要几百毫秒）。因此新建成功后
直接把那一条**插进列表**——元信息用紧接着的一次 `stat` 取真（不是拼出来的近似值），
插完按当前排序键重排、选中它，然后立刻拉起输入框。代价是：新建时若正好有一次扫描在途，
那次扫描会被作废（否则它剩下的批次回来时会与手动插入的这一条重复），列表因此可能
停在半截。这个「欠一次刷新」记在 `ShellView::list_incomplete` 上，
等重命名会话结束（不管有没有真改名）补上。

**可预见的失败都在提交前拦住。** 「名称不能包含字符『?』」「该名称是 Windows 保留的设备名」
这类判断在 `fm-core::naming` 里做完，而不是等系统回一个 `Os { code: 123, .. }`。
平台层只负责一件需要看文件系统才能决定的事：重名怎么办。

### 三处与设计稿的差异

1. **新建之后一律进入重命名**，文件夹与文件都是。设计稿只对文件夹这么做
   （`makeFolder` 进改名，`makeFile` 只弹一条提示）。统一成一种行为是为了不让人去猜
   「为什么建文件夹能直接改名、建文件却不能」，而且带序号的默认名
   （`新建文本文档 (2).txt`）反而更需要马上改——资源管理器也是直接进改名的。
2. **`Esc` 的行为**：组件库的输入框开启 `clean_on_escape()` 后会**清空内容**
   （本项目的搜索框用的是同一套），随后失焦即按放弃处理（空名不会被提交）。
   设计稿的「Esc 直接收起输入框」需要全局键位，而 `crates/app/src/actions.rs`
   目前仍是空壳。
3. **危险色的落点**：确认框的确定按钮用 `ButtonVariant::Danger`，而不是像设计稿那样
   把菜单项本身染红——组件库的菜单项渲染没有危险态变体，为此改成自绘会连带丢掉
   图标列对齐、勾选态与禁用态等现成能力。

### 组件库的一处坑：浮层必须自己挂

`gpui-component` 0.6.4 的 `Root::render` **不渲染通知层与对话框层**——它只渲染
「你的那个视图 + 文字选择层 + 几个 overlay」。这两层必须由**应用自己的视图**挂上：

```rust
.children(gpui_component::Root::render_notification_layer(window, cx))
.children(gpui_component::Root::render_dialog_layer(window, cx))
```

少了这一步的后果很有欺骗性：`push_notification` 与 `open_alert_dialog` 都**静默失效**
——提示和对话框都不出现，也不报错，调用点看起来完全正常。这条约定的出处是库自己的测试
（`dialog/dialog.rs` 里的 `DialogHost` 就是手动调用 `Root::render_dialog_layer(...)` 的）。

本项目的这两层挂在 `ShellView::render` 最外层 div 的末尾（放在最后 = 画在最上层；
两层自身都是 `absolute().inset_0()`，不参与 flex 布局，因此不影响任何区块的尺寸）。
顺带把通知的落点从组件库默认的右上角改成了设计稿的左下角（`left:14px; bottom:40px`）。

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
files-gpui.exe --install-folder-integration  # 安装右键菜单项 + 设为默认打开程序
files-gpui.exe --restore-folder-integration  # 还原全部改动
files-gpui.exe --folder-integration-status   # 打印当前状态（退出码 0）
```

**原生构建，不加 `--target`。** 不需要 VS Developer Prompt，也不需要手动把 SDK 的 bin 注入 `PATH`（原因见下节）。
可通过 `RUST_LOG` 覆盖日志级别，例如 `RUST_LOG=debug cargo run`。

### 构建前必须知道的三件事

**① 运行中的实例会锁住 exe——这是最常见的构建失败。**

Windows 不允许覆盖正在运行的映像，报错形如：

```
error: failed to remove file `...\target\debug\files-gpui.exe`
Caused by: 拒绝访问。 (os error 5)
```

先结束进程再构建。注意**热键宿主是无窗口的后台进程**，在任务管理器里很容易漏掉：

```bash
taskkill /IM files-gpui.exe /F        # 主窗口与热键宿主一起结束
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

安装包的行为约定：

| 项 | 取值 | 理由 |
|---|---|---|
| 安装范围 | 当前用户，`%LOCALAPPDATA%\Programs\Files` | 单文件程序不写任何共享位置，装到 Program Files 只会平白要求管理员权限 |
| 提权 | 全程不弹 UAC | `PrivilegesRequired=lowest` |
| 界面语言 | 简体中文 | 见上 |
| 桌面 / 开始菜单快捷方式 | 默认创建 | |
| 开机自启（热键宿主） | **默认不创建** | 会常驻一个后台进程，应由用户显式选择 |
| 接管文件夹 | **默认不接管** | 会改变「双击文件夹」的系统行为，应由用户显式选择 |
| 安装 / 升级前 | 结束正在运行的实例 | 热键宿主是无窗口进程，Windows 重启管理器看不见它，不主动结束会让覆盖 exe 失败在「文件被占用」 |
| 卸载 | 先还原文件夹接管，再删文件 | 顺序反了会留下指向已删文件的 shell 关联 |
| 用户偏好 `%APPDATA%\files-gpui` | **默认保留**，卸载结束时询问 | 静默卸载会按默认按钮自动回答，所以默认项必须是「否」 |

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

## 文件夹接管的设计与安全性质

### 为什么必须改注册表

Windows **没有「默认文件管理器」这个可设置的项**（设置 → 默认应用里没有这一栏），
也没有任何受支持的 API。唯一可行的办法是覆盖 shell 的文件夹打开处理器，
这是 Files / Total Commander / Directory Opus 这类工具的通行做法。

### 写在哪、怎么还原

全部写在用户级 `HKCU\Software\Classes`：

| 键                                                            | 用途                                                                  |
| ------------------------------------------------------------- | --------------------------------------------------------------------- |
| `Folder\shell`、`Directory\shell`、`Drive\shell` 的**默认值** | 设为`open` —— 双击文件夹/盘符时用的正是这个「默认动词」               |
| `Folder\shell\open\command`                                   | 文件夹的默认打开处理器（工厂原值`%SystemRoot%\Explorer.exe` 在 HKLM） |
| `Directory\shell\open\command`                                | 文件系统文件夹的打开处理器                                            |
| `Drive\shell\open\command`                                    | 盘符的打开处理器（「此电脑」里或桌面上双击`C:` 走它）                 |
| `Directory\shell\Files\command`                               | 右键菜单项，位置参数`%1`（被选中的那个文件夹）                        |
| `Directory\Background\shell\Files\command`                    | 右键菜单项，位置参数`%V`（当前目录；空白处没有选中项）                |

**每个 progid 必须写两条，缺一不可：**

1. `...\shell` 的默认值 = `open` —— 决定「双击时调用哪个动词」；
2. `...\shell\open\command` 的默认值 = 我们的命令行 —— 决定「那个动词执行什么」。

只写第 2 条的后果很有迷惑性：**右键菜单能生效，双击却毫无变化**。
因为右键菜单是显式枚举动词，而双击用的是「默认动词」；工厂状态下
`...\shell` 的默认值是**未设置**的，此时双击由资源管理器内部处理，根本不走 shell 动词。

改完还必须调用 `SHChangeNotify(SHCNE_ASSOCCHANGED, …)` 广播一次，
否则已经在运行的资源管理器会继续用缓存里的旧处理器，必须重启 `explorer.exe` 才生效。

`HKCU\Software\Classes` 是**叠加**在机器级 `HKLM\Software\Classes` 之上的一层，由此得到两条性质：

1. **不需要管理员权限**；
2. **还原只需删掉这些键** —— `HKLM` 里的原始值原封不动，行为自动恢复。

第 2 条是本功能最重要的安全性质：它不依赖「备份回写」这一步是否正确。
备份只在一种情况下用到——用户此前已在 `HKCU` 里自定义过同一个键，
此时会在覆盖前把原值**连同它的值类型**存进 `HKCU\Software\Files\FolderIntegration`
（值类型必须一起存：把 `REG_EXPAND_SZ` 还原成 `REG_SZ` 会让原值里的 `%VAR%` 不再展开）。

### 为什么有 `restore-explorer.bat`

覆盖生效后，若程序被删除或移动，双击文件夹会打不开，而那时程序本身已经跑不起来。
因此安装时会在程序目录旁边写一个纯 `reg delete` 的脚本，即使程序本体损坏也能双击恢复。
脚本内容保持纯 ASCII —— `.bat` 在中文系统上按 GBK 解释，写 UTF-8 中文会显示成乱码。

### 四个曾经写错的地方（已修，并都加了防回归断言）

- **漏写默认动词**（最要命的一个）：只写了 `...\shell\open\command` 而没把
  `...\shell` 的默认值设为 `open`。症状是「右键菜单生效了、双击还是资源管理器」。
  单测现在断言 `shell_key()` 与 `command_key()` 是两个不同的键。
- **动词键与命令子键混淆**：shell 动词键的默认值是**菜单显示文字**，命令在它的
  `\command` 子键里。状态判定曾读成动词键的默认值，于是「注册表明明写对了、
  程序却一直报未安装」，后果是每次开菜单都重复安装一遍。现在由 `CONTEXT_MENU_KEYS`
  一张表驱动安装 / 状态判定 / 卸载三处，并有单测断言「命令键就是动词键的 `\command` 子键」。
- **重复安装污染备份**：没有安装标记时，第二次安装会把**我们自己写的命令**
  当作「用户原值」备份下来，之后还原就把这个命令又写回去 —— 表现是
  「点了还原但文件夹还是打不开」，且全程无声，因为每一步都返回成功。
  现在有 `DefaultHandlerInstalled` 标记，重复安装不会触碰备份。
- **只读句柄上删值 + 还原中途中断**：`RegOpenKeyExW` 只请求了 `KEY_READ`，
  拿这个句柄删值会被拒绝（错误码 5）；而 `restore_all` 原本用 `?` 在第一个错误处
  就返回，于是一次权限失败会让整个还原**半途而废**。对足控场景来说，
  留在「半接管」状态比「有一项没成功」更糟。现在删除走 `open_for_write`，
  且 `restore_all` 逐项尽力还原、收集失败后统一报告。

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

原 `PLAN.md` 记录的是「Linux 交叉编译到 `x86_64-pc-windows-gnu`」路线，该路线已废弃。
本节记录改用 **Windows 原生 MSVC 构建** 后的重新验证结论。

### 决策 1：不创建 `vendor/` 补丁

`PLAN.md` 原方案要求 vendor `gpui-pre-windows-0.3.5` 并对 `directx_renderer.rs` 打补丁，用于绕开两个缺陷。
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

以下来自原 `PLAN.md` 交叉编译方案的内容已全部失效，**照抄会直接导致构建失败**：

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
| 单元测试           | `cargo test --workspace` 62 项全部通过，零警告                                                                                                         |
| release 构建       | 通过：`Finished release profile [optimized] target(s) in 4m 54s`，0 错误，产物 `target/release/files-gpui.exe`（17.8 MB）                              |
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
├── .codebuddy/plans/PLAN.md  # 需求基线与阶段记录
├── design/              # 设计稿（主界面 / 设置页 / 应用图标），界面取值的唯一依据
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
    ├── services/  # 目录流式扫描、图标缓存、配置、监听占位
    └── platform/  # FileSystem trait 及 windows / mock 实现
```

界面与图标的关键字：

| 想改什么                     | 改哪里                                                                 |
| ---------------------------- | ---------------------------------------------------------------------- |
| 颜色、字号、圆角、间距、断点 | `crates/views/src/tokens.rs`（唯一来源）                               |
| 区块尺寸与响应式判定         | `crates/shell/src/layout.rs` 的 `Viewport`                             |
| 某个控件的形态               | `crates/views/src/widgets.rs`（图标按钮 / 分段控件 / 勾选框 / 容量条） |
| 主题与组件库色值同步         | `crates/shell/src/shell.rs` 的 `apply_component_theme`                 |
| 应用图标的几何与配色         | `tools/icon-gen/src/design.rs`，改完重新生成（见 `assets/app-icon/README.md`） |

分层约束：UI 层完全不感知平台差异，所有原生能力经 `platform` 层注入；
`core` 只描述数据与状态流转，不依赖 gpui、Windows API 或文件系统实现。
