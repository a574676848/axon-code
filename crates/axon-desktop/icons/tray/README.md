# 托盘菜单图形

三枚图形的路径**逐字节取自既有工作台产品的图标集**，不手绘、不近似。两套墨色分浅色菜单与深色菜单：
原生菜单跟随的是系统主题而不是应用主题，深色菜单上深墨图标会整片消失，因此按菜单底色各出一套，
运行期按窗口主题选择，并在主题切换时重建菜单（见 `src/desktop_shell.rs`）。

| 用途 | 浅色菜单（深墨） | 深色菜单（浅墨） | 来源图形 |
|---|---|---|---|
| 显示主界面 | `show-on-light.rgba` | `show-on-dark.rgba` | `IconPanelLeftOutlineArtwork` |
| 检查更新 | `update-on-light.rgba` | `update-on-dark.rgba` | `IconRefreshOutlineArtwork` |
| 退出 | `quit-on-light.rgba` | `quit-on-dark.rgba` | `LogoutIcon`（Figma `IcDsLogOutOutline16`） |

来源路径（既有工作台产品仓库内）：

- `packages/client/ui-primitives/src/icons/index.tsx`
- `packages/client/ui-settings-account/src/client/LogoutIcon.tsx`

## 格式

32×32 原始 RGBA：每像素按 R、G、B、A 四字节，行优先，自上而下，共 4096 字节。
直接交给 `tauri::image::Image::new_owned`，**因此不需要任何图像解码依赖**，也不随构建期新增 crate。
墨色取值：浅色菜单用深墨 `#1F1F1F`，深色菜单用浅墨 `#E8E8E8`。

## 再生成方法

1. 取上表来源图形里的 SVG 路径，放进 `viewBox="0 0 16 16"`、`width/height=32` 的画布；
   描边类图形线宽 1（`stroke-width="1"`），`LogoutIcon` 是填充路径，按其原有平移与缩放摆放。
2. 用画布栅格化成 32×32 位图，再把像素导成原始 RGBA。
   窗体自身的 WebView 就能做这件事：用 `tools/cdp-eval.ps1` 执行一段 canvas 栅格化并读回像素，
   无需引入构建期图形依赖。
3. 逐字节写入 `*-on-light.rgba` 与 `*-on-dark.rgba`；尺寸由
   `desktop_shell.rs` 的测试 `tray_icon_assets_match_the_declared_size` 守住。
