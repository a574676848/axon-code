//! Windows 平台适配器：经 Shell 的**关联处理程序枚举**拿到「用其他应用打开」候选。
//!
//! 与 DSH 参考实现（`file-applications-windows.ts`）逐条对齐：
//!
//! 1. `SHAssocEnumHandlers(扩展名, ASSOC_FILTER_NONE)` 取枚举器；
//! 2. 逐个 `IAssocHandler` 调 `GetName`（标识）、`GetUIName`（展示名）、`GetIconLocation`（图标位置）；
//! 3. `AssocQueryStringW` 分别取 `ASSOCSTR_EXECUTABLE` 与 `ASSOCSTR_APPID` 作为「系统默认」的判据；
//! 4. 按标识去重（保留先出现者），用可执行文件名兜底展示名。
//!
//! 图标提取在 `windows_icon`；本文件只管枚举与标识。
//!
//! # 为什么必须 unsafe
//!
//! Shell 的关联处理程序只能经 COM 接口获得，windows-rs 生成的接口方法都是 `unsafe fn`
//! （调用方要保证接口指针有效、出参缓冲可写、返回的字符串所有权被接管）。本文件的每个
//! `unsafe` 块都带 `// SAFETY:` 说明对应不变量。

use std::path::Path;

// `::windows`（而不是 `windows`）显式指向外部 crate：本 crate 自己也有一个名为 `windows`
// 的模块，加上前缀可免除歧义。
use ::windows::{
    Win32::{
        Foundation::{HWND, RPC_E_CHANGED_MODE, S_OK},
        System::Com::{
            COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize, IDataObject,
        },
        UI::Shell::{
            ASSOC_FILTER_NONE, ASSOCF_NONE, ASSOCSTR, ASSOCSTR_APPID, ASSOCSTR_EXECUTABLE,
            AssocQueryStringW, BHID_DataObject, IAssocHandler, IShellItem, SHAssocEnumHandlers,
            SHCreateItemFromParsingName,
        },
        UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
        },
    },
    core::{PCWSTR, PWSTR},
};

use crate::{
    FileApplication,
    windows_helpers::{eq_ignore_case, shell_extension, to_wide},
    windows_icon,
};

/// Shell 启动的等待预算上限。
///
/// 处理程序可能在自己的异步启动工作释放引用之前就返回，因此调用后要泵一小段消息循环；
/// 这是上限而不是固定时长：消息队列安静下来就立刻结束（见 [`pump_shell_messages`]）。
const SHELL_LAUNCH_PUMP_BUDGET_MS: u64 = 2_000;

/// 连续多少次空轮询后认定启动已经交棒。
const SHELL_LAUNCH_QUIET_POLLS: u32 = 5;

/// 空轮询之间的间隔。
const SHELL_LAUNCH_POLL_MS: u64 = 10;

/// 枚举某个文件的候选打开方式。
///
/// 无扩展名时直接返回空表（`SHAssocEnumHandlers` 只按扩展名查关联，没有扩展名就没有可查的键）。
pub(crate) fn applications_for(path: &Path) -> Result<Vec<FileApplication>, String> {
    let Some(extension) = shell_extension(path) else {
        return Ok(Vec::new());
    };
    // 关联查询走 Shell 的 COM 对象；套间在此进入，守卫离开作用域时按需退出。
    let _apartment = ComApartment::enter()?;
    let wide_extension = to_wide(&extension);
    let extension_pointer = PCWSTR(wide_extension.as_ptr());

    let default_executable = associated_string(extension_pointer, ASSOCSTR_EXECUTABLE);
    let default_app_id = associated_string(extension_pointer, ASSOCSTR_APPID);

    let mut candidates = Vec::new();
    visit_handlers(&extension, |handler| {
        if let Some(candidate) = describe(
            &handler,
            default_executable.as_deref(),
            default_app_id.as_deref(),
        ) {
            candidates.push(candidate);
        }
        true
    })?;
    Ok(candidates)
}

/// 用系统登记的一条打开方式打开文件。
///
/// 标识就是 [`applications_for`] 给出的 `id`（`IAssocHandler::GetName` 的原文）：重新枚举该
/// 扩展名的处理程序，找到标识相同的那一条，再经 Shell 的数据对象调用它的 `Invoke`。
/// 这与 DSH 的 Windows 适配器走同一条路径：不猜可执行文件，也不自行拼命令行；
/// 标识不在该文件的关联列表里时如实报错，绝不退回「随便找一个」。
pub(crate) fn open_with(path: &Path, application_id: &str) -> Result<(), String> {
    let Some(extension) = shell_extension(path) else {
        return Err("该文件没有扩展名，系统没有登记任何打开方式".to_string());
    };
    if !path.is_file() {
        return Err(format!("目标不是文件，无法用它打开：{}", path.display()));
    }
    let _apartment = ComApartment::enter()?;
    let handler = find_handler(&extension, application_id)?;

    let wide_path = to_wide(&path.to_string_lossy());
    // SAFETY: `wide_path` 是以 NUL 结尾的宽字符串且活到本函数结束；`None` 表示不借用任何绑定
    // 上下文。返回的 `IShellItem` 由 windows-rs 的接口类型持有，析构时 Release。
    let item: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(wide_path.as_ptr()), None) }
        .map_err(|error| format!("无法为文件建立 Shell 项：{error}"))?;
    // SAFETY: `item` 是上面成功返回的有效接口；`BHID_DataObject` 是 Shell 公开定义的处理程序
    // 绑定标识。返回的数据对象由 `IDataObject` 持有，析构时 Release。
    let data_object: IDataObject = unsafe { item.BindToHandler(None, &BHID_DataObject) }
        .map_err(|error| format!("无法取得 Shell 数据对象：{error}"))?;
    // SAFETY: `handler` 与 `data_object` 都是本线程上存活的有效接口，数据对象的生命周期覆盖
    // 本次调用；`Invoke` 只读取它并转交给处理程序。
    unsafe { handler.Invoke(&data_object) }
        .map_err(|error| format!("用 {application_id} 打开失败：{error}"))?;

    // 处理程序可能已经返回，而真正的启动仍在 Shell 里异步进行：把这段时间的消息泵完再走，
    // 否则进程一退出就把还没交棒完的引用计数拆掉，用户看到的是「点了没反应」。
    pump_shell_messages();
    Ok(())
}

/// 遍历某个扩展名的关联处理程序；回调返回 `false` 即提前停止。
///
/// 取不出一条 handler 属于单项失败：按降级原则结束遍历，而不是让整张列表失败。
fn visit_handlers(
    extension: &str,
    mut visit: impl FnMut(IAssocHandler) -> bool,
) -> Result<(), String> {
    let wide_extension = to_wide(extension);
    let extension_pointer = PCWSTR(wide_extension.as_ptr());
    // SAFETY: `extension_pointer` 指向上面的 `wide_extension`，是以 NUL 结尾的宽字符串且活到
    // 本函数结束；`shell_extension` 保证扩展名以点开头，符合 `SHAssocEnumHandlers` 对
    // `pszextra` 的要求。返回的枚举器由 windows-rs 的接口类型持有（失败时是 `Err`，
    // 不会是无效指针），析构时 Release。
    let enumerator = unsafe { SHAssocEnumHandlers(extension_pointer, ASSOC_FILTER_NONE) }
        .map_err(|error| format!("枚举 {extension} 的关联处理程序失败：{error}"))?;
    loop {
        let mut batch: [Option<IAssocHandler>; 1] = [None];
        let mut fetched = 0_u32;
        // SAFETY: `enumerator` 是上面成功返回的有效枚举器；`batch` 与 `fetched` 都是本栈帧上
        // 已初始化的局部变量，`Next` 按 `batch.len()`（= 1）最多写入一个接口指针。
        // 被写入的接口由 windows-rs 的 `IAssocHandler` 持有，离开作用域即 Release。
        let next = unsafe { enumerator.Next(&mut batch, Some(&mut fetched)) };
        if next.is_err() || fetched == 0 {
            break;
        }
        let Some(handler) = batch[0].take() else {
            break;
        };
        if !visit(handler) {
            break;
        }
        // `handler` 在此析构：windows-rs 生成的接口类型在 Drop 时调用 Release。
    }
    Ok(())
}

/// 按标识找回一条处理程序；标识不在该扩展名的关联列表里时如实报错。
fn find_handler(extension: &str, application_id: &str) -> Result<IAssocHandler, String> {
    let mut found = None;
    visit_handlers(extension, |handler| {
        // SAFETY: `handler` 是本线程上存活的有效接口（`visit_handlers` 刚交出、尚未析构）。
        // `GetName` 失败时不接管任何指针；成功时返回值是待释放的 CoTaskMem 字符串。
        let name = unsafe {
            handler
                .GetName()
                .ok()
                .and_then(|value| take_com_string(value))
        };
        if name.is_some_and(|name| eq_ignore_case(&name, application_id)) {
            found = Some(handler);
            return false;
        }
        true
    })?;
    found.ok_or_else(|| format!("系统没有为这类文件登记这个打开方式：{application_id}"))
}

/// 泵一小段消息循环，等 Shell 的异步启动交棒。
///
/// 结束条件是「消息队列连续安静若干次」而不是固定时长：启动通常几十毫秒内就交棒，
/// 只有处理程序真的还在忙时才吃满预算。
fn pump_shell_messages() {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(SHELL_LAUNCH_PUMP_BUDGET_MS);
    let mut quiet = 0_u32;
    let mut message = MSG::default();
    while std::time::Instant::now() < deadline && quiet < SHELL_LAUNCH_QUIET_POLLS {
        // SAFETY: `message` 是本栈帧上已初始化的局部变量，`PeekMessageW` 只在取到消息时写入它；
        // 传 `PM_REMOVE` 表示取走这条消息，因此下面不会重复派发同一条。
        let has_message =
            unsafe { PeekMessageW(&mut message, Some(HWND::default()), 0, 0, PM_REMOVE) }.as_bool();
        if !has_message {
            quiet += 1;
            std::thread::sleep(std::time::Duration::from_millis(SHELL_LAUNCH_POLL_MS));
            continue;
        }
        quiet = 0;
        // SAFETY: `message` 刚被 `PeekMessageW` 填成一条完整消息，两个调用都只读它。
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// 把一条处理程序转成候选；取不到标识时返回 `None`，由调用方跳过它。
fn describe(
    handler: &IAssocHandler,
    default_executable: Option<&str>,
    default_app_id: Option<&str>,
) -> Option<FileApplication> {
    // SAFETY: `handler` 由调用方保证是本线程上存活的有效接口指针（`Next` 刚写入、尚未析构）。
    // `GetName` 失败时不接管任何指针；成功时返回值是待释放的 CoTaskMem 字符串。
    // 标识为空说明这条 handler 无法被回传（「用哪个应用打开」无从表达），直接跳过。
    let id = unsafe { take_com_string(handler.GetName().ok()?) }.filter(|id| !id.is_empty())?;
    // 展示名是可选信息：取不到就回退到可执行文件名，与 Windows 自身「打开方式」列表的降级一致。
    // SAFETY: 同 `GetName`；`GetUIName` 失败时只是没有展示名，不会接管任何指针，
    // 成功时返回值同样是待释放的 CoTaskMem 字符串。
    let ui_name = unsafe {
        handler
            .GetUIName()
            .ok()
            .and_then(|value| take_com_string(value))
    };
    let name = ui_name
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| file_name_of(&id));
    // 默认项判据与 DSH 一致：标识等于系统登记的可执行文件，或等于系统登记的 AppID。
    let default = [default_executable, default_app_id]
        .into_iter()
        .flatten()
        .any(|default| eq_ignore_case(&id, default));
    // 图标在 `id` 被移动进结构体之前取用。
    let icon = windows_icon::data_url(handler, &id);
    Some(FileApplication {
        id,
        name,
        icon,
        default,
    })
}

/// 接管一个由 COM 分配的宽字符串：复制成 Rust `String` 后用 `CoTaskMemFree` 归还。
///
/// 返回 `None` 表示调用方给的是空指针，或字符串含孤立代理项（无法解码成合法 UTF-16 文本）。
///
/// # Safety
///
/// `value` 必须是 COM 按 `CoTaskMemAlloc` 约定分配、以 NUL 结尾的宽字符串指针，且**尚未被
/// 释放**：本函数无条件接管它的所有权，包括解码失败的分支（失败也不会漏释放）。
pub(crate) unsafe fn take_com_string(value: PWSTR) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: 由调用方保证 `value` 指向以 NUL 结尾的有效宽字符串；`to_string` 读到 NUL 为止，
    // 只读不写，也不保留指针。
    let text = unsafe { value.to_string() }.ok();
    // SAFETY: `value` 的所有权已按契约转移给本函数；这里恰好释放一次，且不在任何失败分支上跳过。
    // `value` 已排除空指针，因此 `Some(..)` 一定是有效指针。
    unsafe { CoTaskMemFree(Some(value.0.cast_const().cast())) };
    text
}

/// 查询 Shell 为某扩展名登记的关联串（默认可执行文件、AppID 等）。
///
/// 两次调用是 `AssocQueryStringW` 的约定：首次传空输出缓冲**只问所需长度**（此时返回的是
/// `S_FALSE`，不是错误），第二次才真正取值。查不到关联同样不是错误：返回 `None`，
/// 于是没有候选会被标记为默认项，而不是让整张列表失败。
fn associated_string(extension: PCWSTR, kind: ASSOCSTR) -> Option<String> {
    let mut size = 0_u32;
    // SAFETY: 首次调用按约定传空输出缓冲与有效的长度指针；`extension` 由调用方保证是以 NUL
    // 结尾的宽字符串。本次返回值只表示「缓冲够不够」，此处刻意忽略。
    let _ = unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
            kind,
            extension,
            PCWSTR::null(),
            None,
            &mut size,
        )
    };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0_u16; size as usize];
    // SAFETY: `buffer` 恰好 `size` 个 `u16`，与上一步查询到的需求一致，写入不越界；
    // `buffer` 活到本次调用之后。
    let result = unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
            kind,
            extension,
            PCWSTR::null(),
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut size,
        )
    };
    if result.is_err() {
        return None;
    }
    // 用有损解码而不是严格解码：路径里出现孤立代理项时宁可显示替换字符，也不要丢掉这条默认项。
    Some(String::from_utf16_lossy(truncate_at_nul(&buffer)))
}

/// 取宽字符串缓冲里第一个 NUL 之前的内容。
///
/// 成功返回的长度含结尾 NUL，且缓冲通常比实际内容大（`vec![0; size]` 已保证未写入部分为 0），
/// 因此按第一个 NUL 截断而不是按长度截断。
fn truncate_at_nul(buffer: &[u16]) -> &[u16] {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    &buffer[..end]
}

/// `GetUIName` 取不到时的回退展示名：标识的文件名部分。
fn file_name_of(id: &str) -> String {
    Path::new(id)
        .file_name()
        .map_or_else(|| id.to_owned(), |name| name.to_string_lossy().into_owned())
}

/// 本线程的 COM 套间守卫。
///
/// Shell 的关联处理程序以 COM 对象形式提供，调用线程必须先进入套间（DSH 的参考实现在
/// `-STA` 的 PowerShell 进程里执行同一批 API）。[`applications_for`] 是同步函数，可能在任意
/// 线程上被调用，因此这里按需初始化，并按返回值决定是否**拥有**该套间：
///
/// - `S_OK`：本次调用完成了初始化，本守卫拥有它，析构时必须配对 `CoUninitialize`。
/// - `S_FALSE`：该线程此前已被初始化（模式相同），反初始化归别的所有者，本守卫不能碰。
/// - `RPC_E_CHANGED_MODE`：该线程已是多线程套间，无法改成单线程。此时**不报错、继续枚举**：
///   注册表来源的关联在 MTA 下同样能枚举出来，而拒绝服务会让整个「打开方式」入口空白。
///   代价是打包应用（MSIX）的处理器可能不出现：这是已知缺口，靠冒烟报告观察。
/// - 其他失败：返回错误，由调用方决定如何提示。
struct ComApartment {
    /// 本守卫是否拥有该套间（即析构时是否需要调用 `CoUninitialize`）。
    owned: bool,
}

impl ComApartment {
    /// 让当前线程进入单线程套间（能给就给）。
    fn enter() -> Result<Self, String> {
        // SAFETY: `pvReserved` 按 MSDN 规定必须为 null，这里正是 null；调用只影响本线程的
        // COM 状态，返回值在本函数内判定完毕，不需要调用方提供任何不变量。
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result.is_ok() {
            return Ok(Self {
                owned: result == S_OK,
            });
        }
        if result == RPC_E_CHANGED_MODE {
            return Ok(Self { owned: false });
        }
        Err(format!("进入 COM 单线程套间失败：{result}"))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: 只有 `CoInitializeEx` 在本线程返回 `S_OK` 时 `owned` 才为真，即本守卫是
            // 该线程唯一的初始化所有者，因此这里恰好配对一次。
            unsafe { CoUninitialize() };
        }
    }
}
