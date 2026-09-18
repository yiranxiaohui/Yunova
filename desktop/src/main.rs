//! Yunova desktop client.
//!
//! Turns the user's own computer into an execution target, and — since this is
//! a desktop app and not a service — shows the product while doing it. The
//! window embeds the site's own UI, so chatting, starting a task and watching
//! it run happen here rather than in a browser next to a terminal.
//!
//! Underneath, the client dials the server and holds a WebSocket open: a
//! personal machine usually has no reachable address, so an outbound
//! connection is the only thing that works without port forwarding. Over that
//! socket it runs `pi --mode rpc` locally and pipes its stdio through. The
//! thinking loop is the runtime's, the session and billing are the server's,
//! and the local execution policy (workspace scope, approval gate) is this
//! client's. That split is what lets a task started on a phone run here
//! without the phone or the server being trusted with the machine.
//!
//! Attaching is a sign-in, not a pairing step: the user gives their account
//! credentials once and the server binds this machine on the spot. What stays
//! on disk afterwards is a device-scoped token, so the credential this client
//! holds can be revoked to one computer instead of standing in for the
//! account.
//!
//! Two front ends, one connector. The window is the default because it is what
//! a person installs; `--headless` keeps the original terminal behaviour for
//! servers and development machines, where there is no display to open a
//! window on. Both drive [`connector::Connector`], so the protocol and the
//! local policy exist once.

mod cli;
mod connector;
mod endpoint;
mod identity;
mod proto;
mod runtime;
#[path = "../../src/runtime_env.rs"]
mod runtime_env;
mod runtime_install;
mod settings;
#[cfg(not(target_os = "android"))]
mod shell;
#[cfg(not(target_os = "android"))]
mod updates;

fn main() {
    // rustls 0.23 cannot pick a backend on its own; without this, connecting
    // to wss:// panics.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => usage(),
        Some("--version" | "-V") => println!("yunova-desktop {}", env!("CARGO_PKG_VERSION")),
        // No window: a server or a CI box has no display, and the old
        // environment-driven behaviour is exactly right there.
        Some("--headless") => cli::run(),
        // Write the approval gate this build would install, and exit.
        //
        // Exists for `tests/approval_gate_renders.rs`, which runs the
        // generated JavaScript under node the way pi does. The generator lives
        // in a binary crate with no lib target, so there is no other way for a
        // test to get at the exact file that ships — and a gate asserted only
        // as a Rust string is how it came to be shipped with a dialog that
        // rendered `{}`. Not documented in `usage`: it configures nothing and
        // is not something a user has a reason to run.
        Some("--emit-approval-gate") => emit_approval_gate(&args[1..]),
        _ => shell::run(),
    }
}

/// `--emit-approval-gate <mode> <dir>`: write the gate and stop.
fn emit_approval_gate(args: &[String]) {
    let [mode, dir] = args else {
        eprintln!("用法: yunova-desktop --emit-approval-gate <always|commands|never> <目录>");
        std::process::exit(2);
    };
    let Some(mode) = runtime::ApprovalMode::parse(mode) else {
        eprintln!("无法识别的审批方式: {mode}");
        std::process::exit(2);
    };
    let dir = std::path::PathBuf::from(dir);
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to start the async runtime")
        .block_on(runtime::write_runtime_config(
            &dir,
            &serde_json::json!({ "providers": {} }),
            mode,
        ));
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn usage() {
    println!(
        "Yunova 桌面客户端 {}\n\
         \n\
         用法:\n\
         \x20 yunova-desktop              打开桌面应用（内置站点界面 + 本机执行端）\n\
         \x20 yunova-desktop --headless   无界面模式，供服务器/开发机使用\n\
         \n\
         无界面模式的环境变量:\n\
         \x20 YUNOVA_DEVICE_URL           站点地址，默认连接本应用内置的站点\n\
         \x20 YUNOVA_USERNAME/PASSWORD    免交互登录\n\
         \x20 YUNOVA_DEVICE_WORKSPACE     Agent 可操作的目录，默认当前目录\n\
         \x20 YUNOVA_DEVICE_NAME          设备名，默认主机名\n\
         \x20 YUNOVA_DEVICE_AUTO_APPROVE  审批方式：always（默认，逐条确认）、\n\
         \x20                             commands（只确认命令）、never（全部放行）。\n\
         \x20                             仍兼容旧的 1/0 写法，1 等于 never，谨慎使用\n\
         \x20 YUNOVA_PI_BIN               运行时可执行文件，默认 pi\n\
         \n\
         桌面模式的设置保存在 OS 配置目录，可在应用内的「本机设置」中修改。\n\
         桌面模式会在启动后检查更新，安装由「本机设置」里的按钮触发；\n\
         无界面模式不自更新，请用包管理器或重新下载压缩包升级。",
        env!("CARGO_PKG_VERSION")
    );
}
