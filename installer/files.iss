; Pagefold · Windows 安装包定义
;
; ── 构建方式 ────────────────────────────────────────────────────────────────
;   1) 先出发布产物（安装包的载荷）：
;        cargo build --release -p fm-app
;   2) 用 Inno Setup 的命令行编译器编译本脚本：
;        target\innosetup\ISCC.exe installer\files.iss
;      产物落在 dist\Pagefold-<版本>-x64-setup.exe
;
;   Inno Setup 本体不需要安装：官方安装器支持免管理员的 portable 解包，
;   解到 target\innosetup 即可（命令见 README「打包安装程序」一节）。
;
; ── 设计要点 ────────────────────────────────────────────────────────────────
;   · **用户级安装**：装到 %LOCALAPPDATA%\Programs\Pagefold，全程不弹 UAC。
;     PrivilegesRequired=lowest 让安装全程不弹 UAC——装到 Program Files 需要管理员
;     权限，而本程序是单文件、不写共享位置，装到系统目录没有任何收益。
;   · **对系统有副作用的动作一律默认不勾选**：接管文件夹会改变「双击文件夹」的行为，
;     开机自启会常驻一个后台进程。两者都做成可选任务，默认关闭，用户显式勾选才生效。
;   · **卸载先还原、再删文件**：文件夹接管写的是注册表，必须在程序本体被删除之前
;     用程序自己的 `--restore-folder-integration` 还原，否则会留下一堆指向不存在
;     文件的 shell 关联（表现为双击文件夹报错）。顺序在 [UninstallRun] 里，不可调换。
;   · **安装前先结束进程**：热键宿主是无窗口的后台进程，Windows 的重启管理器看不到它，
;     不主动结束的话覆盖 exe 会失败在「文件被占用」上。

; 应用名。它就是快捷方式名、控制面板里的显示名，
; 也是任务栏上显示的窗口标题（两处必须一致，否则用户会看到两个名字）。
#define AppName "Pagefold"
#define AppExeName "pagefold.exe"
#define AppPublisher "Pagefold"

; 版本号的默认值与 Cargo.toml 的 workspace.package.version 保持一致（本机构建用）。
; CI 打 tag 时传 `/DAppVersion=x.y.z /DAppVersionQuad=x.y.z.0` 覆盖，
; 因此必须用 `#ifndef` 包起来：ISPP 对已定义的符号再次 `#define` 会直接报错。
#ifndef AppVersion
  #define AppVersion "0.5.3"
#endif
#ifndef AppVersionQuad
  #define AppVersionQuad "0.5.3.0"
#endif

; 载荷与图标：路径相对本脚本所在目录解析。
#define SourceExe "..\target\release\" + AppExeName
#define SourceIcon "..\assets\app-icon\windows\icon.ico"

[Setup]
; AppId 是升级与卸载的唯一定位依据，**发布之后绝不能再改**，
; 否则新版本会被当成另一个程序、与旧版本并存。下面的值是随机生成的固定值。
AppId={{0C19383B-C097-44EB-8360-FC08FDFA3853}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
VersionInfoVersion={#AppVersionQuad}
DefaultDirName={localappdata}\Programs\Pagefold
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; 只有一个可选的安装目录（就是上面那个），没有让用户改的必要，直接跳过。
DisableDirPage=yes
PrivilegesRequired=lowest
; GPUI 走 Direct3D 11 渲染，Windows 10 是事实下限。
MinVersion=10.0
; 只用 x64 产物：载荷是 x86_64-pc-windows-msvc 构建的。
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=Pagefold-{#AppVersion}-x64-setup
SetupIconFile={#SourceIcon}
UninstallDisplayIcon={app}\{#AppExeName}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; 安装前结束正在运行的实例（见文件头的说明），不做「关闭后自动重启」。
CloseApplications=yes
RestartApplications=no

[Languages]
; Default.isl 打底、中文覆盖：这样即便中文语言文件比编译器版本旧、
; 缺少个别新消息，也只会退回英文，而不会编译失败。
Name: "chinese"; MessagesFile: "compiler:Default.isl,languages\ChineseSimplified.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; Flags: checkedonce
Name: "startup"; Description: "开机自动启动（让 Win+E 随时可用）"; Flags: unchecked
Name: "folderintegration"; Description: "接管文件夹：设为默认打开方式，并加入右键菜单"; Flags: unchecked

[Files]
Source: {#SourceExe}; DestDir: "{app}"; Flags: ignoreversion
; 说明：`restore-explorer.bat` 不在这里提供——它由程序在安装文件夹接管时自己写到
; 程序目录旁边（见 fm-platform 的 folder_association），随接管的安装/卸载成对增删。

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon
; 开机自启的是**热键宿主**这个模式，不是主窗口：它只跑托盘图标与键盘钩子，
; 常驻内存几 MB，主窗口仍是「打开才存在」。
Name: "{userstartup}\{#AppName} 热键宿主"; Filename: "{app}\{#AppExeName}"; Parameters: "--hotkey-host"; Tasks: startup

[Run]
; 文件夹接管交给程序自己做：要写哪些键只有它清楚（见 fm-platform::folder_association）。
Filename: "{app}\{#AppExeName}"; Parameters: "--install-folder-integration"; StatusMsg: "正在接管文件夹…"; Flags: runhidden waituntilterminated; Tasks: folderintegration
Filename: "{app}\{#AppExeName}"; Description: "立即启动 {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; 顺序不可调换：
; 1) 先结束进程，否则下一步的程序无法启动、且后续删除文件会被占用；
Filename: "{cmd}"; Parameters: "/C taskkill /IM {#AppExeName} /F >NUL 2>&1"; Flags: runhidden waituntilterminated; RunOnceId: "StopPagefold"
; 2) 再用程序自己的命令还原全部 shell 改动（右键菜单项 + 默认打开程序 + 安装标记）；
Filename: "{app}\{#AppExeName}"; Parameters: "--restore-folder-integration"; Flags: runhidden waituntilterminated; RunOnceId: "RestoreFolderIntegration"

[UninstallDelete]
; 程序目录里还有运行期生成的文件（接管文件夹时写的 restore-explorer.bat），
; 它们不是 Inno 安装的，不会被自动清掉，因此整个目录一起删。
; 用户偏好设置**不在这里删**，它在别处、且是否保留该由用户决定（见 [Code]）。
Type: filesandordirs; Name: "{app}"

[Code]
// 安装前结束正在运行的实例。
//
// 为什么不能只靠 Inno 的 CloseApplications：那个走 Windows 重启管理器，
// 而热键宿主是 DETACHED 的无窗口进程，重启管理器不会把它算进来；
// 结果就是「主窗口被关掉了，但 exe 仍被宿主占用」，覆盖文件失败。
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
begin
  Exec(
    ExpandConstant('{cmd}'),
    '/C taskkill /IM {#AppExeName} /F >NUL 2>&1',
    '',
    SW_HIDE,
    ewWaitUntilTerminated,
    ResultCode
  );
  // 返回空串表示「继续安装」。taskkill 的退出码在这里无意义：
  // 没有进程在跑时它是 128，属于正常情况。
  Result := '';
end;

// 卸载结束后询问是否删除偏好设置。
//
// 默认**保留**：配置里放的是主题、视图模式、密度这类用户调过的东西，
// 重装后能接着用；但想彻底清干净的用户也应该有个入口。
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  ConfigDir: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    ConfigDir := ExpandConstant('{userappdata}\pagefold');
    if DirExists(ConfigDir) then
    begin
      // MB_DEFBUTTON2 让**默认按钮是「否」**。这不是随手选的：
      // 静默卸载（/SUPPRESSMSGBOXES）会拿默认按钮自动回答，
      // 若默认是「是」，`unins000.exe /VERYSILENT` 就会在不打招呼的情况下
      // 抹掉用户主题与视图偏好——自动化场景下这种「数据静默丢失」最难追。
      if MsgBox(
        '是否同时删除偏好设置？' + #13#10 + #13#10 + ConfigDir,
        mbConfirmation,
        MB_YESNO or MB_DEFBUTTON2
      ) = IDYES then
      begin
        DelTree(ConfigDir, True, True, True);
      end;
    end;
  end;
end;
