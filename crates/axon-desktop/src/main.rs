//! Axon Workbench 桌面端执行入口。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// 解析 `--serve <端口> [静态目录]` 参数。
fn parse_serve_args(args: &[String]) -> Option<(u16, Option<String>)> {
    let position = args.iter().position(|arg| arg == "--serve")?;
    let port = args
        .get(position + 1)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(14222);
    let root = args.get(position + 2).cloned();
    Some((port, root))
}

fn main() {
    // 切档中继参数必须在任何 Tauri 初始化与单实例注册之前识别并消费掉：
    // 中继不是一个界面进程，它不建窗口、不注册插件，因此不会被判成第二实例。
    let (relay_for_pid, _) = axon_desktop::connection::relaunch::take_relaunch_argument(
        std::env::args_os().skip(1).collect(),
    );
    if let Some(old_pid) = relay_for_pid {
        let launcher = match axon_desktop::connection::relaunch::resolve_launcher() {
            Ok(launcher) => launcher,
            Err(reason) => {
                eprintln!("ERROR: 中继无法解析稳定入口：{reason}");
                std::process::exit(1);
            }
        };
        let code = axon_desktop::connection::relaunch::run_relay(old_pid, &launcher);
        std::process::exit(code);
    }

    let args: Vec<String> = std::env::args().collect();

    if let Some((port, root)) = parse_serve_args(&args) {
        // 调试桥零鉴权、分派表覆盖全部命令，且它的响应契约只有 200 加
        // `{ok:false,error}`，没有状态码通道，因此不能靠「返回 403」拦，
        // 只能在云档案生效时直接拒绝启动。
        if axon_desktop::connection::active_is_cloud() {
            eprintln!("拒绝启动调试桥：当前生效的是云端档案，调试桥只允许在本机档案下使用");
            std::process::exit(2);
        }
        if let Err(error) = axon_desktop::dev_server::serve(port, root) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }

    if args.iter().any(|arg| arg == "--self-check") {
        match axon_desktop::self_check::run_self_check() {
            Ok(result) => {
                let json = serde_json::to_string(&result)
                    .unwrap_or_else(|_| "{\"success\":true}".to_string());
                println!("{json}");
                std::process::exit(0);
            }
            Err(err) => {
                eprintln!("{{\"success\":false,\"error\":\"{err}\"}}");
                std::process::exit(1);
            }
        }
    }

    axon_desktop::run();
}
