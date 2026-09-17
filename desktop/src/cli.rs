//! The headless front end.
//!
//! Kept because the window is not always the right answer: a server, a build
//! box or a container has no display, and there the environment-driven start
//! that this client originally had is exactly what is wanted. It drives the
//! same [`Connector`], so nothing about the protocol or the local policy is
//! duplicated here — only the way a person is told what is happening.

use std::path::PathBuf;
use std::sync::Arc;

use crate::connector::{Connector, ConnectorConfig, Host, Status};
use crate::endpoint::{default_site_url, hostname};
use crate::identity::{Login, prompt_login};
use crate::runtime_env;

/// Prints, because a terminal is the whole UI here.
struct TerminalHost;

impl Host for TerminalHost {
    fn status(&self, status: Status) {
        match status {
            Status::Offline => eprintln!("[device] 已断开"),
            Status::Connecting => eprintln!("[device] 正在连接…"),
            Status::Online {
                device_id,
                username,
            } => println!("[device] 已连接，设备 {device_id}（{username}）"),
            Status::NeedsLogin { reason } => match reason {
                Some(r) => eprintln!("[device] 需要登录: {r}"),
                None => eprintln!("[device] 需要登录"),
            },
            Status::Error { message } => eprintln!("[device] {message}"),
        }
    }

    fn log(&self, line: String) {
        eprintln!("[device] {line}");
    }

    fn attention(&self, title: String, body: String) {
        // No notification centre to post to, so it goes where the operator is
        // already looking. Still worth saying loudly: an unattended headless
        // client with approvals on will otherwise appear to hang.
        eprintln!("[device] ⚠ {title}: {body}");
    }

    fn login(&self) -> Result<Login, String> {
        prompt_login(
            runtime_env::var("YUNOVA_USERNAME").ok(),
            runtime_env::var("YUNOVA_PASSWORD").ok(),
        )
    }
}

pub fn run() {
    // The address is the product's own, so a headless run needs no
    // configuration either; `YUNOVA_DEVICE_URL` remains for self-hosted
    // instances. There is no window here to read a session from, so this path
    // still signs in with an account.
    let raw = runtime_env::var("YUNOVA_DEVICE_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default_site_url().to_string());

    // The workspace bounds what the agent can reach. Defaulting to the current
    // directory rather than $HOME keeps an unconfigured run from exposing
    // everything the user owns.
    let workspace = runtime_env::var("YUNOVA_DEVICE_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let state_dir = runtime_env::var("YUNOVA_DEVICE_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace.join(".yunova-agent"));
    let auto_approve = matches!(
        runtime_env::var("YUNOVA_DEVICE_AUTO_APPROVE")
            .unwrap_or_default()
            .as_str(),
        "1" | "true" | "yes" | "on"
    );

    let config = ConnectorConfig {
        site_url: raw,
        name: runtime_env::var("YUNOVA_DEVICE_NAME").unwrap_or_else(|_| hostname()),
        workspace,
        state_dir,
        program: runtime_env::var("YUNOVA_PI_BIN").unwrap_or_else(|_| "pi".into()),
        auto_approve,
        // The credential lives outside the workspace: the workspace is
        // precisely what the agent may rewrite, and a token the agent can edit
        // is a token it can replace or leak. An explicit override exists for
        // packaging.
        config_dir: runtime_env::var("YUNOVA_DEVICE_CONFIG_DIR")
            .ok()
            .map(PathBuf::from),
    };

    println!("Yunova 桌面客户端（无界面模式）");
    println!(
        "  服务器:   {}",
        crate::endpoint::device_endpoint(&config.site_url)
    );
    println!("  设备名:   {}", config.name);
    println!("  工作目录: {}", config.workspace.display());
    println!(
        "  审批:     {}",
        if config.auto_approve {
            "已放开（Agent 可直接执行命令）"
        } else {
            "逐条确认（在网页或手机上处理）"
        }
    );
    if config.auto_approve {
        println!("  ⚠ 自动批准下 Agent 可在本机任意执行命令，只在信任的环境中使用。");
    }
    match crate::connector::bound_account(&config) {
        Some(c) if !c.username.is_empty() => {
            println!("  账号:     {}（设备 {}）", c.username, c.device_id)
        }
        Some(_) => println!("  账号:     已保存设备凭证"),
        None => println!("  账号:     未登录，马上要求登录"),
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start the async runtime");

    runtime.block_on(async move {
        let connector = Connector::new(Arc::new(TerminalHost));

        // Stop local runtimes on Ctrl-C rather than orphaning them.
        let shutdown = Arc::clone(&connector);
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            eprintln!("\n正在停止本机运行时…");
            shutdown.stop().await;
            std::process::exit(0);
        });

        connector.start(config, None).await;
        // The connector owns its own retry loop and returns only when it has
        // given up for a reason retrying cannot fix, so watching for that is
        // what keeps the process alive without a second supervision loop here.
        while connector.is_running().await {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        // Exit non-zero: a service unit must see a failed start as a failure
        // rather than as a clean shutdown it should not restart.
        if let Status::NeedsLogin { .. } | Status::Error { .. } = connector.status().await {
            connector.stop().await;
            std::process::exit(1);
        }
        connector.stop().await;
    });
}
