#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! `tela-gui`：GPUI 桌面入口。
//!
//! 职责只有三行：装配 [`tela_app::services::AppServices`] → 转成 `tela-ui` 需要的
//! 只读数据源 → 启动窗口。业务/视图逻辑分别在 `tela-app` / `tela-ui`。
//!
//! 运行：
//! ```bash
//! cargo run -p tela-app --features gui --bin tela-gui           # 真实 API（登录模式）
//! cargo run -p tela-app --features gui --bin tela-gui -- --demo # 演示数据，无需账号
//! TELA_DB=target/tela-gui.db cargo run -p tela-app --features gui --bin tela-gui
//! ```
//!
//! 默认是真实 API 装配：发布包内置 public client id，未登录时侧栏显示「登录 Tela」。
//! `--demo`（或 `TELA_DEMO=1`）才进入进程内 mock 的演示模式。
//!
//! `--check` 是无显示器冒烟：只装配服务、读一次列表，不创建窗口，供 CI 验证
//! “数据链路 + 演示 seed”可用（配合 `--demo`）。
//!
//! release 构建在 Windows 上不再弹控制台；启动失败会同时写到数据目录的
//! `tela-gui-error.log`（`%APPDATA%\tela\` 或 `$XDG_DATA_HOME/tela/`）。

use std::sync::Arc;

use tela_app::services::{AppConfig, AppServices};
use tela_proto::TelaError;
use tela_ui::account::AccountService;
use tela_ui::gui::{GuiApp, run};
use tela_ui::shell::{RecommendationService, ShellData};

/// 命令行参数。
struct Args {
    demo: bool,
    check: bool,
}

const USAGE: &str = "\
用法：tela-gui [--demo] [--check]

默认进入真实 API（登录）模式：未登录时侧栏显示「登录 Tela」。
  --demo       使用进程内 mock 与演示数据，不需要账号
  --check      无窗口冒烟：装配并读一次列表后退出（CI 用）
  -h, --help   显示本帮助";

fn parse_args() -> Args {
    let mut args = Args {
        demo: false,
        check: false,
    };
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--demo" => args.demo = true,
            "--check" => args.check = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => {
                eprintln!("tela-gui: 未知参数 {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    args
}

fn main() {
    let args = parse_args();
    if args.check {
        check(args.demo);
        return;
    }

    let services = match AppServices::bootstrap_gui(args.demo) {
        Ok(services) => services,
        Err(error) => fatal("启动失败", &error),
    };

    // 真实装配在开窗前同步一轮：窗口先画本地库，随后由 CLI/后续切片继续刷新。
    // demo 装配没有登录态，直接跳过（is_connected() 为 false）。
    if services.is_connected() && services.client_id().is_none_or(str::is_empty) {
        eprintln!(
            "tela-gui: 已登录但缺少 TELA_CLIENT_ID，跳过启动同步；\
             设置 TELA_CLIENT_ID 后重启（发布包应在构建时内置）"
        );
    } else if services.is_connected() {
        match services.sync_once() {
            Ok(report) => eprintln!(
                "tela-gui: 启动同步完成 rows={} pending={}",
                report.pull.rows,
                report.push.map(|push| push.remaining).unwrap_or(0)
            ),
            Err(error) => eprintln!("tela-gui: 启动同步失败（继续读本地库）：{error}"),
        }
    }
    let services = Arc::new(services);

    // `Arc<AppServices>` 同时满足两种用途：作为 `ShellData` 只读数据源，
    // 以及通过 `content()` 把正文 API 交给阅读面板。
    let data: Arc<dyn ShellData> = services.clone();
    let accounts: Arc<dyn AccountService> = services.clone();
    let recommendations: Arc<dyn RecommendationService> = services.clone();
    let content = services.content();
    let offline_cache = services.offline_cache();
    run(GuiApp::with_accounts(data, content, accounts)
        .with_offline_cache(offline_cache)
        .with_recommendations(recommendations));
}

/// 启动失败：终端 + 数据目录里的日志各留一份，然后按错误码退出。
///
/// Windows 的 release 构建没有控制台（`windows_subsystem = "windows"`），
/// 崩溃/启动失败时用户看不到 stderr，只能靠这个日志定位。
fn fatal(context: &str, error: &TelaError) -> ! {
    let message = format!("tela-gui: {context}：{error}");
    eprintln!("{message}");

    let path = AppConfig::from_env()
        .db_path
        .with_file_name("tela-gui-error.log");
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, format!("{message}\n"));

    std::process::exit(error.exit_code());
}

fn check(demo: bool) {
    let services = match AppServices::bootstrap_gui(demo) {
        Ok(services) => services,
        Err(error) => fatal("--check 装配失败", &error),
    };

    let feeds = match services.list_feeds() {
        Ok(feeds) => feeds,
        Err(error) => fatal("--check 订阅列表失败", &error),
    };
    let mut articles = 0usize;
    for feed in &feeds {
        match services.list_articles(feed.id) {
            Ok(rows) => articles += rows.len(),
            Err(error) => fatal(&format!("--check 文章列表失败（feed {}）", feed.id), &error),
        }
    }
    let unread: usize = feeds.iter().map(|feed| feed.unread).sum();
    let recommendations = match services.list_recommendations() {
        Ok(rows) => rows.len(),
        Err(error) => fatal("--check 推荐列表失败", &error),
    };
    let mode = if services.auth().is_some() {
        "real"
    } else {
        "demo"
    };
    println!(
        "tela-gui check: mode={mode} feeds={} articles={} unread={} recommendations={recommendations}",
        feeds.len(),
        articles,
        unread
    );
}
