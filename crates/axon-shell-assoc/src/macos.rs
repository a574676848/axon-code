//! macOS 平台适配器：经 LaunchServices 枚举「用其他应用打开」候选，并回放选中的那一条。
//!
//! 查询走系统自带的 `osascript -l JavaScript`（JXA），与既有工作台产品的参考实现
//! （`file-applications.ts`）同一条路径：脚本只读 `NSWorkspace` 的公开 API，
//! **不引入任何 macOS 专用 FFI 依赖**，因此本 crate 在 Windows / Linux 上照样编译得过，
//! 也不会留下编不过也跑不了的死代码。
//!
//! 分工：
//!
//! - `macos_applications`：JSON 形状校验、同应用多份拷贝的折叠、版本比较：纯逻辑，任何平台可测；
//! - 本文件：真正只在 macOS 上跑的两个子进程调用（查询、启动）。
//!
//! 与 DSH 一致的两条边界：
//!
//! 1. **启动前重新校验**：`open_with` 会再查一次（这次不要图标，因此很快），
//!    确认这个标识确实登记在这个文件上，然后才 `open -a`。界面交出的标识不能直接当命令用。
//! 2. **绝不改动系统默认应用**：选择只作用于这一次打开。

use std::path::Path;
use std::process::Command;

use crate::FileApplication;
use crate::macos_applications::{self, MacApplication};

/// 查询脚本：把 LaunchServices 的候选与（可选的）图标编码成 JSON。
///
/// 第二个参数是 `icons` 或 `handlers`：只要标识与展示名时传后者，省掉逐个取图的开销。
const APPLICATIONS_SCRIPT: &str = r#"
ObjC.import('AppKit');
function run(argv) {
  var workspace = $.NSWorkspace.sharedWorkspace;
  var file = $.NSURL.fileURLWithPath(argv[0]);
  var preferred = workspace.URLForApplicationToOpenURL(file);
  var preferredPath = preferred.isNil() ? null : ObjC.unwrap(preferred.path);
  var urls = workspace.URLsForApplicationsToOpenURL(file);
  var apps = [];
  for (var i = 0; i < urls.count; i++) {
    var url = urls.objectAtIndex(i);
    var path = ObjC.unwrap(url.path);
    var image = null;
    if (argv[1] === 'icons') {
      var icon = workspace.iconForFile(path);
      var thumbnail = $.NSImage.alloc.initWithSize($.NSMakeSize(32, 32));
      thumbnail.lockFocus;
      icon.drawInRectFromRectOperationFraction($.NSMakeRect(0, 0, 32, 32), $.NSZeroRect, $.NSCompositingOperationSourceOver, 1);
      thumbnail.unlockFocus;
      var bitmap = $.NSBitmapImageRep.imageRepWithData(thumbnail.TIFFRepresentation);
      var png = bitmap.representationUsingTypeProperties($.NSBitmapImageFileTypePNG, $({}));
      image = png.isNil() ? null : 'data:image/png;base64,' + ObjC.unwrap(png.base64EncodedStringWithOptions(0));
    }
    var bundle = $.NSBundle.bundleWithURL(url);
    var bundleId = bundle.isNil() || bundle.bundleIdentifier.isNil() ? null : ObjC.unwrap(bundle.bundleIdentifier);
    var version = bundle.isNil() ? null : bundle.objectForInfoDictionaryKey('CFBundleShortVersionString');
    apps.push({
      id: path,
      name: ObjC.unwrap($.NSFileManager.defaultManager.displayNameAtPath(path)),
      default: path === preferredPath,
      icon: image,
      bundle: bundleId,
      version: version === null || version.isNil() ? null : String(ObjC.unwrap(version))
    });
  }
  return JSON.stringify(apps);
}
"#;

/// 一次查询里最多保留的候选数：菜单用不了那么多，而每个图标都是一份 base64。
const MAX_CANDIDATES: usize = 40;

/// 枚举某个文件的候选打开方式。
pub(crate) fn applications_for(path: &Path) -> Result<Vec<FileApplication>, String> {
    let mut queried = query(path, "icons")?;
    // 图标很贵：超出的候选直接丢掉，而不是排到最后却没有图标：菜单里
    // 「有图标的排在前面、没图标的垫底」会让系统自己的优先级看起来是乱的。
    queried.truncate(MAX_CANDIDATES);
    Ok(macos_applications::dedupe_applications(queried)
        .into_iter()
        .map(|application| FileApplication {
            id: application.id,
            name: application.name,
            icon: application.icon,
            default: application.default,
        })
        .collect())
}

/// 用某个应用打开文件。
pub(crate) fn open_with(path: &Path, application_id: &str) -> Result<(), String> {
    // 重新查一遍确认这个标识确实登记在这个文件上：界面交出的标识不是命令。
    let registered = query(path, "handlers")?;
    if !registered
        .iter()
        .any(|application| application.id == application_id)
    {
        return Err(format!(
            "系统没有为这类文件登记这个打开方式：{application_id}"
        ));
    }
    let status = Command::new("open")
        .arg("-a")
        .arg(application_id)
        .arg(path)
        .status()
        .map_err(|error| format!("无法启动 open：{error}"))?;
    if !status.success() {
        return Err(format!("用 {application_id} 打开失败"));
    }
    Ok(())
}

/// 执行一次查询；失败时把 `osascript` 的说明原样带出来。
fn query(path: &Path, mode: &str) -> Result<Vec<MacApplication>, String> {
    let output = Command::new("osascript")
        .arg("-l")
        .arg("JavaScript")
        .arg("-e")
        .arg(APPLICATIONS_SCRIPT)
        .arg(path)
        .arg(mode)
        .output()
        .map_err(|error| format!("无法执行 osascript：{error}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        return Err(format!("查询打开方式失败：{}", message.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    macos_applications::parse_applications(&stdout)
        .ok_or_else(|| "打开方式查询返回了无法解析的结果".to_string())
}
