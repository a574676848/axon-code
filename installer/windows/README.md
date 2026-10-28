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
  -o "dist/axon-v$version-x86_64-pc-windows-msvc.msi"
```

`WixToolset.UI.wixext` 是 `WixUI_Advanced` 向导的依赖，`zh-CN.wxl` 固定安装向导为简体中文，`IconPath` 是快捷方式与
「应用和功能」列表图标的来源：两个参数缺一个都构不出安装包，手动构建与 CI 必须
传同一组参数。

前置二进制由以下命令产出：

```powershell
cargo build -p axon-cli -p axon-launcher -p axon-desktop --release --target x86_64-pc-windows-msvc --all-features
```

本地构建未传版本参数时，脚本应从仓库根 `Cargo.toml` 的 `[workspace.package].version` 读取；发布构建使用 Git tag，并要求与该版本一致。

## CI 构建

在 `.github/workflows/release.yml` 的 `msi` job 中自动执行，
产物上传到 GitHub Release 的 Assets。

## 安装包特性

| 特性 | 说明 |
|------|------|
| 安装范围 | Per-User（无需管理员权限） |
| 安装目录 | `%USERPROFILE%\.axon\bin\` |
| PATH 写入 | 用户级 PATH，加入 `%USERPROFILE%\.axon\bin`，新终端自动生效 |
| 卸载 | 控制面板 → 程序 → 卸载 Axon |
| 升级 | `MajorUpgrade` 就地升级，`current.json` 切换指向新版本目录 |
| 静默安装 | `msiexec /i axon.msi /quiet` |

## 配方不变量（改动前必读）

1. **MSI 只写入程序目录 `%USERPROFILE%\.axon\bin`，不接管配置和运行数据。** 用户配置、Soul、identity 和运行态数据由 Axon 自己在用户目录维护；安装末尾的 `axon-launcher.exe daemon install` 只补齐缺失的默认 Soul，绝不覆盖用户内容。
2. **`axon.cmd` 的 Source 按仓库根解析**（`installer\windows\axon.cmd`），与其余四个路径参数保持一致，因此构建必须从仓库根发起。
3. **程序组件随 MSI 生命周期管理**：卸载会删除 `axon.exe`、`axon-desktop.exe`、launcher、`axon.cmd` 和版本目录；配置、Soul、identity 与运行数据位于 `.axon` 的其他目录中，不会被安装器清理。
4. **安装末尾会执行 `axon-launcher.exe daemon install`**，即注册当前用户计划任务并播种 `SOUL.md`。这是安装包的既有副作用，静默安装同样生效；`Return="ignore"` 表示该动作失败不阻断安装。

## winget 配方参考

发布 Windows 安装包时，可向 `microsoft/winget-pkgs` 仓库提交对应版本的清单：
`manifests/a/AlexZ/Axon/<version>/`

## 安装向导与快捷方式

配方使用 `WixUI_Advanced`：它在 per-user 安装下同时提供**安装路径选择**与**功能勾选**，是内置向导里唯一能同时满足这两件事的。

- **安装路径**：向导里的安装位置可改；静默安装用 `msiexec /i axon.msi /quiet INSTALLLOCATION=<路径>`。
- **桌面快捷方式**：做成独立功能 `DesktopShortcut`，安装时可勾掉；同时会在开始菜单建 `Axon` 目录。
- **图标**：`crates/axon-desktop/icons/icon.ico` 同时用于快捷方式与「应用和功能」列表（`ARPPRODUCTICON`）。
- **构建时需多带一个扩展**：`-ext WixToolset.UI.wixext`，并额外传入 `-d IconPath=crates\axon-desktop\icons\icon.ico`。

### 关于「固定到任务栏」

配方**故意不做**任务栏固定。Windows 自 Windows 10 起不再向第三方提供可用的固定接口：`TaskbarManager.RequestPinCurrentAppAsync` 只对 MSIX 打包应用有效，向 `Quick Launch\User Pinned\TaskBar` 写快捷方式会被 Explorer 重写掉，重启 Explorer 才能生效且属于侵入性操作。

成熟做法（VS Code、Chrome 同类安装器）是：安装器建好开始菜单与桌面快捷方式，用户在**首次启动后**自行右键任务栏图标固定，或从开始菜单右键固定。要改成一键固定，唯一可靠路径是把应用改成 MSIX 打包。
