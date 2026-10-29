//! `tela-gui`：GPUI 桌面入口。
//!
//! 职责只有三行：装配 [`tela_app::services::AppServices`] → 转成 `tela-ui` 需要的
//! 只读数据源 → 启动窗口。业务/视图逻辑分别在 `tela-app` / `tela-ui`。
//!
//! 运行：
//! ```bash
//! TELA_DB=target/tela-gui.db cargo run -p tela-app --features gui --bin tela-gui
//! ```
//!
//! `--check` 是无显示器冒烟：只装配服务、读一次列表，不创建窗口，供 CI 验证
//! “数据链路 + 演示 seed”可用。
//!
//! `TELA_DEMO=0` 时改用真实 API 与本地库；默认仍是 demo（开箱即用）。

use std::sync::Arc;

use tela_ui::account::AccountService;
use tela_ui::gui::{GuiApp, run};
use tela_ui::shell::ShellData;

fn main() {
    if std::env::args().skip(1).any(|arg| arg == "--check") {
        check();
        return;
    }

    let services = match tela_app::services::AppServices::bootstrap_gui() {
        Ok(services) => services,
        Err(error) => {
            eprintln!("tela-gui: 启动失败：{error}");
            std::process::exit(error.exit_code());
        }
    };

    // 真实装配在开窗前同步一轮：窗口先画本地库，随后由 CLI/后续切片继续刷新。
    // demo 装配没有登录态，直接跳过（is_connected() 为 false）。
    if services.is_connected() && services.client_id().is_none_or(str::is_empty) {
        eprintln!(
            "tela-gui: 已登录但缺少 TELA_CLIENT_ID，跳过启动同步；\
             先加载本地配置（source tmp/tela.env）或设置 TELA_CLIENT_ID"
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
    let content = services.content();
    run(GuiApp::with_accounts(data, content, accounts));
}

fn check() {
    let services = match tela_app::services::AppServices::bootstrap_gui() {
        Ok(services) => services,
        Err(error) => {
            eprintln!("tela-gui --check: 装配失败：{error}");
            std::process::exit(error.exit_code());
        }
    };

    let feeds = match services.list_feeds() {
        Ok(feeds) => feeds,
        Err(error) => {
            eprintln!("tela-gui --check: 订阅列表失败：{error}");
            std::process::exit(error.exit_code());
        }
    };
    let mut articles = 0usize;
    for feed in &feeds {
        match services.list_articles(feed.id) {
            Ok(rows) => articles += rows.len(),
            Err(error) => {
                eprintln!(
                    "tela-gui --check: 文章列表失败（feed {}）：{error}",
                    feed.id
                );
                std::process::exit(error.exit_code());
            }
        }
    }
    let unread: usize = feeds.iter().map(|feed| feed.unread).sum();
    println!(
        "tela-gui check: feeds={} articles={} unread={}",
        feeds.len(),
        articles,
        unread
    );
}
