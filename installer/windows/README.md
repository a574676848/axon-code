# WiX MSI 构建说明

本目录包含 Axon Windows 安装包配方。

## 前置条件

- .NET SDK 8.0+
- WiX Toolset v5：
  ```
  dotnet tool install --global wix --version 5.0.2
  ```

## 手动构建

**必须从仓库根目录执行**，配方中的路径按仓库根解析：

```powershell
$version            = "<版本号>"
$binaryPath         = "target\x86_64-pc-windows-msvc\release\axon.exe"
$desktopBinaryPath  = "target\x86_64-pc-windows-msvc\release\axon-desktop.exe"
$launcherPath       = "target\x86_64-pc-windows-msvc\release\axon-launcher.exe"
$manifestPath       = "dist\current.json"
@{ versionId = $version } | ConvertTo-Json -Compress | Set-Content -NoNewline $manifestPath

wix build installer/windows/axon.wxs `
  -ext WixToolset.UI.wixext `
  -culture zh-CN `
  -loc installer/windows/zh-CN.wxl `
  -d "Version=$version" `
  -d "BinaryPath=$binaryPath" `
  -d "DesktopBinaryPath=$desktopBinaryPath" `
  -d "LauncherPath=$launcherPath" `
  -d "CurrentManifestPath=$manifestPath" `
  -d "IconPath=crates\axon-desktop\icons\icon.ico" `
  -o "dist/axon-$version-x86_64-pc-windows-msvc.msi"
```

`WixToolset.UI.wixext` 是 `WixUI_Advanced` 向导的依赖，`zh-CN.wxl` 固定安装向导为简体中文，`IconPath` 是快捷方式与
「应用和功能」列表图标的来源：两个参数缺一个都构不出安装包，手动构建与 CI 必须
传同一组参数。

前置二进制由以下命令产出：

```powershell
cargo build -p axon-cli -p axon-launcher -p axon-desktop --release --target x86_64-pc-windows-msvc --all-features
```

本地构建未传版本参数时，脚本应从仓库根 `Cargo.toml` 的 `[workspace.package].version` 读取；发布构建使用 Git tag，并要求与该版本一致。

## 语义校验

构建产物可用 WiX 自带的 ICE 校验确认配方语义：

```powershell
wix msi validate "dist/axon-<版本号>-x86_64-pc-windows-msvc.msi"
```

它检查目录残留、组件 KeyPath、快捷方式指向等 Windows Installer 规则。改动配方后必须先跑通再提交。

开始菜单目录位于用户配置目录内，因此快捷方式组件带 `RemoveFolder`（`On="uninstall"`）满足 ICE64；删掉它会让校验直接失败并报 `AxonMenuDir` 未列入 RemoveFile 表。

## CI 构建

在 `.github/workflows/release.yml` 的 `msi` job 中自动执行，
产物上传到 GitHub Release 的 Assets。

## 安装包特性

| 特性 | 说明 |
|------|------|
| 安装范围 | Per-User（无需管理员权限） |
| 安装目录 | `%USERPROFILE%\.axon\bin\` |
| 组件选择 | 核心必装，后端与桌面端分别可勾选；静默安装用 `ADDLOCAL` 指定 |
| PATH 写入 | 用户级 PATH，加入 `%USERPROFILE%\.axon\bin`，新终端自动生效 |
| 卸载 | 控制面板 → 程序 → 卸载 Axon |
| 升级 | `MajorUpgrade` 就地升级，`current.json` 切换指向新版本目录 |
| 静默安装 | `msiexec /i axon.msi /quiet` |

## 组件矩阵

四个 Feature，组件归属唯一：每个组件只被一个 Feature 引用，不依赖「同一组件被两个 Feature 同时引用」这种未验证行为。

| Feature | ComponentGroup | 组件 | 安装语义 |
|---|---|---|---|
| `AxonCore` | `AxonCoreComponents` | `AxonLauncher`、`AxonCurrentManifest`、`AxonCommand`、`AxonPath`、`AxonAppUserModelId`（通知身份） | 必装，不可取消 |
| `AxonBackend` | `AxonBackendComponents` | `AxonExe` | 可选 |
| `AxonDesktop` | `AxonDesktopComponents` | `AxonDesktopExe`、`AxonDesktopShortcut` | 可选 |
| `DesktopShortcut` | 无 | 无 | 空 Feature，仅保留 ID 维持 `MajorUpgrade` 的 Feature 状态映射，向导中不显示 |

三个组件组的组级 `Directory` 都是 `VERSIONDIR`（`versions\<版本号>`），需要别的位置的组件显式声明 `Directory`：launcher 落 `LAUNCHERDIR`，`current.json` 与 `axon.cmd` 落 `INSTALLDIR`，快捷方式落 `SHORTCUTDIR`。

`AxonPath` **必须显式声明 `Directory`**：它不依赖组级目录继承，PATH 的值取自 `INSTALLDIR`，而组级目录是 `VERSIONDIR`，两者不是同一个位置。

静默安装按组件裁剪：

```powershell
# 只装桌面端：不注册 Daemon 服务，不创建计划任务
msiexec /i axon.msi /quiet ADDLOCAL=AxonCore,AxonDesktop

# 只装后端：不铺桌面二进制，安装末尾注册 Daemon 用户服务
msiexec /i axon.msi /quiet ADDLOCAL=AxonCore,AxonBackend
```

`AxonCore` 不可从 `ADDLOCAL` 中省略。

## 配方不变量（改动前必读）

1. **MSI 只写入程序目录 `%USERPROFILE%\.axon\bin`，不接管配置和运行数据。** 用户配置、Soul、identity 和运行态数据由 Axon 自己在用户目录维护；安装末尾的 `axon-launcher.exe daemon install` 只补齐缺失的默认 Soul，绝不覆盖用户内容。配方里不存在指向配置、`skills/`、identity 与运行数据目录的写入。
2. **`axon.cmd` 的 Source 按仓库根解析**（`installer\windows\axon.cmd`），与其余四个路径参数保持一致，因此构建必须从仓库根发起。
3. **程序组件随 MSI 生命周期管理**：卸载会删除已安装 Feature 内的 `axon.exe`、`axon-desktop.exe`、launcher、`axon.cmd`、`current.json` 与版本目录，并删除开始菜单里为空的 `Axon` 目录；配置、Soul、identity 与运行数据位于 `.axon` 的其他目录中，不会被安装器清理。
4. **安装末尾会执行 `axon-launcher.exe daemon install`**，即注册当前用户计划任务并播种 `SOUL.md`，条件是 `&AxonBackend = 3`（后端组件被选中）。`Return="ignore"` 表示该动作失败不阻断安装，因此**不得用安装退出码判断服务注册成功**，必须用 `axon daemon status` 或计划任务列表核验。只装桌面端或只装核心时不执行该动作，不会留下后台常驻任务。
5. **`AxonCore` 的必装语义**用 WiX 5 接受的 `AllowAbsent="no"` 表达，落到 MSI Feature 表就是 `DisallowAbsent`（Attributes `0x10`），与 `Absent="disallow"` 是同一语义。WiX 5 不接受 Feature 上的 `Absent` 属性写法。
6. **`AxonCore` 与 `DesktopShortcut` 的 ID 不可改名或删除**：`MajorUpgrade` 的 Feature 状态迁移按 ID 匹配，换 ID 会中断既有安装的升级映射。`DesktopShortcut` 已不承载组件，只保留 ID。

## winget 配方参考

发布 Windows 安装包时，可向 `microsoft/winget-pkgs` 仓库提交对应版本的清单：
`manifests/a/AlexZ/Axon/<version>/`

## 安装向导与快捷方式

配方使用 `WixUI_Advanced`：它在 per-user 安装下同时提供**安装路径选择**与**功能勾选**，是内置向导里唯一能同时满足这两件事的。

- **安装路径**：向导里的安装位置可改；静默安装用 `msiexec /i axon.msi /quiet INSTALLLOCATION=<路径>`。
- **桌面快捷方式**：随「Axon 桌面工作台」（`AxonDesktop`）一并安装，勾掉该功能即不创建；同时会在开始菜单建 `Axon` 目录。`DesktopShortcut` 是保留 ID 的空 Feature，向导中不显示。
- **图标**：`crates/axon-desktop/icons/icon.ico` 同时用于快捷方式与「应用和功能」列表（`ARPPRODUCTICON`）。
- **构建时需多带一个扩展**：`-ext WixToolset.UI.wixext`，并额外传入 `-d IconPath=crates\axon-desktop\icons\icon.ico`。

### 关于「固定到任务栏」

配方**故意不做**任务栏固定。Windows 自 Windows 10 起不再向第三方提供可用的固定接口：`TaskbarManager.RequestPinCurrentAppAsync` 只对 MSIX 打包应用有效，向 `Quick Launch\User Pinned\TaskBar` 写快捷方式会被 Explorer 重写掉，重启 Explorer 才能生效且属于侵入性操作。

成熟做法（VS Code、Chrome 同类安装器）是：安装器建好开始菜单与桌面快捷方式，用户在**首次启动后**自行右键任务栏图标固定，或从开始菜单右键固定。要改成一键固定，唯一可靠路径是把应用改成 MSIX 打包。
