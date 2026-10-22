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
    let args: Vec<String> = std::env::args().collect();

    if let Some((port, root)) = parse_serve_args(&args) {
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
