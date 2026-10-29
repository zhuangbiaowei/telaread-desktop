//! `tela-app` 终端入口：登录、同步、列表、阅读、演示。
//!
//! 用法总览见 `tela-app --help`；真实 API 的最小闭环：
//!
//! ```bash
//! export TELA_CLIENT_ID=tela_app_…
//! tela-app login
//! tela-app sync
//! tela-app list
//! tela-app read 101
//! ```
//!
//! 默认（不带子命令）仍是进程内演示阅读页，不需要账号与网络。

use std::io::{self, BufRead, Write};
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tela_app::services::{AppConfig, AppServices, DEFAULT_LANG, TokenBackend};
use tela_content::api::ContentApi;
use tela_content::mock::MOCK_CONTENT_KEY;
use tela_sync::PullOptions;
use tela_ui::reader::ViewMode;
use tela_ui::reader::page::render_session;
use tela_ui::reader::session::{DEFAULT_PAGE_SIZE, ReaderSession};

/// Tela 桌面客户端。
#[derive(Debug, Parser)]
#[command(
    name = "tela-app",
    version,
    about = "Tela 桌面客户端（真实 API v1：OAuth 登录 / 同步 / 阅读）",
    long_about = None
)]
struct Cli {
    /// 子命令；省略时进入离线演示阅读页。
    #[command(subcommand)]
    command: Option<Command>,

    /// 译文语言：zh-Hans | zh-Hant | en | fr
    #[arg(long, global = true)]
    lang: Option<String>,

    /// 阅读视图：original | compare | translation
    #[arg(long, global = true, default_value = "compare")]
    view: String,

    /// 每页段落数
    #[arg(long, global = true, default_value_t = DEFAULT_PAGE_SIZE)]
    page_size: usize,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    /// 用浏览器完成 OAuth 授权，并把刷新令牌写进系统凭据库
    Login {
        /// 不自动打开浏览器，只打印授权地址
        #[arg(long)]
        no_browser: bool,
        /// 刷新令牌只放内存（不落盘，退出即失效）
        #[arg(long)]
        ephemeral: bool,
    },
    /// 撤销令牌并删除本地账号数据
    Logout,
    /// 查看本地同步状态与今天的额度
    Status {
        /// 不访问网络，只显示本地状态
        #[arg(long)]
        offline: bool,
    },
    /// 推送离线队列并拉取最新数据
    Sync {
        /// 丢掉游标，强制从快照开始
        #[arg(long)]
        full: bool,
        /// 只拉这些行类型（逗号分隔），例如 articles,states
        #[arg(long)]
        types: Option<String>,
    },
    /// 列出订阅或某个订阅下的文章
    List {
        /// 只看这个 feed 的文章
        #[arg(long)]
        feed: Option<i64>,
        /// 只显示未读文章
        #[arg(long)]
        unread: bool,
    },
    /// 打开一篇文章（原文 / 对照 / 译文）
    Read {
        /// 文章 id
        id: i64,
        /// 打开前请求正文翻译（需要 write 令牌）
        #[arg(long)]
        translate: bool,
        /// 打开时不自动标记已读（默认遵守 reader.mark_on_open）
        #[arg(long)]
        no_mark_read: bool,
    },
    /// 把文章标记为已读（入队，随后推送）
    MarkRead {
        /// 文章 id
        id: i64,
    },
    /// 把文章标记为未读（入队，随后推送）
    MarkUnread {
        /// 文章 id
        id: i64,
    },
    /// 点赞或取消点赞（入队，随后推送）
    Like {
        /// 文章 id
        id: i64,
        /// 取消点赞
        #[arg(long)]
        unlike: bool,
    },
    /// 查看服务端能力与限额（无需登录）
    Meta,
    /// 离线演示阅读页（不需要账号与网络）
    Demo {
        /// 正文 key 或场景别名：mock / mock-long / mock-empty / mock-partial
        #[arg(long, default_value = MOCK_CONTENT_KEY)]
        chapter: String,
    },
}

fn main() {
    let cli = Cli::parse();

    let Some(mode) = ViewMode::parse(&cli.view) else {
        eprintln!(
            "tela-app: 视图参数非法：{:?}（可选 original | compare | translation）",
            cli.view
        );
        std::process::exit(2);
    };

    match cli.command.clone().unwrap_or(Command::Demo {
        chapter: MOCK_CONTENT_KEY.to_string(),
    }) {
        Command::Demo { chapter } => run_demo(&cli, mode, &chapter),
        Command::Login {
            no_browser,
            ephemeral,
        } => run_login(&cli, no_browser, ephemeral),
        Command::Logout => run_logout(&cli),
        Command::Status { offline } => run_status(&cli, offline),
        Command::Sync { full, types } => run_sync(&cli, full, types),
        Command::List { feed, unread } => run_list(&cli, feed, unread),
        Command::Read {
            id,
            translate,
            no_mark_read,
        } => run_read(&cli, mode, id, translate, no_mark_read),
        Command::MarkRead { id } => {
            run_mutation(&cli, tela_proto::Mutation::mark_read(id), "markRead")
        }
        Command::MarkUnread { id } => {
            run_mutation(&cli, tela_proto::Mutation::mark_unread(id), "markUnread")
        }
        Command::Like { id, unlike } => run_mutation(
            &cli,
            tela_proto::Mutation::set_liked(id, !unlike),
            if unlike {
                "setLiked(false)"
            } else {
                "setLiked(true)"
            },
        ),
        Command::Meta => run_meta(&cli),
    }
}

fn lang_of(cli: &Cli) -> String {
    cli.lang
        .clone()
        .filter(|lang| !lang.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LANG.to_string())
}

fn real_services(cli: &Cli, ephemeral: bool) -> AppServices {
    let mut config = AppConfig::from_env();
    config.lang = lang_of(cli);
    if ephemeral {
        config.token_backend = TokenBackend::Memory;
    }
    match AppServices::bootstrap_real_with(config) {
        Ok(services) => services,
        Err(error) => fail(&error),
    }
}

fn run_demo(cli: &Cli, mode: ViewMode, chapter: &str) {
    let api: Arc<dyn ContentApi> = Arc::from(tela_content::api::mock_content_api());
    let lang = lang_of(cli);
    let mut session = ReaderSession::new(
        api,
        tela_content::api::ChapterRequest::new(chapter.to_string(), lang),
    )
    .with_page_size(cli.page_size);
    session.set_mode(mode);
    render_and_interact(session, None);
}

fn run_login(cli: &Cli, no_browser: bool, ephemeral: bool) {
    let config = AppConfig::from_env();
    if config.client_id.trim().is_empty() {
        eprintln!(
            "tela-app: 缺少 TELA_CLIENT_ID。\
             先在 Tela 注册应用，把 `tela_app_…` 配进环境变量（参考 docs/real-api.md）。"
        );
        std::process::exit(10);
    }
    let services = real_services(cli, ephemeral);
    if services.is_connected() {
        println!("已经连接；如需换账号，先运行 `tela-app logout`。");
    }

    println!("正在监听 127.0.0.1 的回调端口，并准备打开系统浏览器…");
    let me = match services.login(!no_browser, Duration::from_secs(600), |url| {
        println!("授权地址（浏览器未打开时手动访问）：\n{url}\n");
    }) {
        Ok(me) => me,
        Err(error) => fail(&error),
    };
    println!(
        "已连接：@{}（scope={}）{}",
        me.handle,
        me.scope,
        if let Some(email) = &me.email {
            format!("，{email}")
        } else {
            String::new()
        }
    );
    if ephemeral {
        println!("注意：--ephemeral 只把刷新令牌放在内存，本进程退出后需重新登录。");
    }
}

fn run_logout(cli: &Cli) {
    let services = real_services(cli, false);
    match services.logout() {
        Ok(()) => println!("已登出；本地账号数据已清空。"),
        Err(error) => {
            eprintln!("已删除本地凭据，但撤销请求失败：{error}");
        }
    }
}

fn run_status(cli: &Cli, offline: bool) {
    let services = real_services(cli, false);
    let status = match services.sync_status() {
        Ok(status) => status,
        Err(error) => fail(&error),
    };
    print!("{}", status.render());

    if offline {
        return;
    }
    if !services.is_connected() {
        println!("login    (未登录；运行 `tela-app login`)");
        if let Some(error) = services.credential_error() {
            println!("keyring  {error}");
        }
        return;
    }
    match services.me() {
        Ok(me) => println!("login    @{}（scope={}）", me.handle, me.scope),
        Err(error) => println!("login    (检查失败：{error})"),
    }
    match services.usage() {
        Ok(usage) => {
            let quota = |value: Option<i64>| match value {
                Some(value) => value.to_string(),
                None => "不限".to_string(),
            };
            println!(
                "today    translations {}/{} · feeds +{}/{} · posts {}/{} · subscriptions {}/{}",
                usage.translations.used_today,
                quota(usage.translations.per_day),
                usage.feeds.added_today,
                quota(usage.feeds.per_day),
                usage.posts.opened_today,
                quota(usage.posts.per_day),
                usage.subscriptions.count,
                quota(usage.subscriptions.max),
            );
        }
        Err(error) => println!("today    (额度查询失败：{error})"),
    }
}

fn run_sync(cli: &Cli, full: bool, types: Option<String>) {
    let services = real_services(cli, false);
    if !services.is_connected() {
        fail(&tela_proto::TelaError::NotLoggedIn {
            detail: "没有可用的 Tela 凭据".to_string(),
        });
    }
    if full {
        if let Err(error) = services.reset_cursor() {
            fail(&error);
        }
        println!("已丢掉本地游标，本次从快照开始。");
    }
    let mut opts = PullOptions::new(lang_of(cli));
    if let Some(types) = types {
        opts = opts.with_types(types.split(',').map(str::trim).filter(|s| !s.is_empty()));
    }

    match services.sync_with(&opts) {
        Ok(report) => print_sync_report(&report),
        Err(error) => {
            eprintln!("tela-app: 同步失败：{error}");
            std::process::exit(error.exit_code());
        }
    }
}

fn print_sync_report(report: &tela_sync::SyncReport) {
    if let Some(push) = &report.push {
        println!(
            "push     batches={} sent={} applied={} rejected={} remaining={}",
            push.batches,
            push.sent,
            push.applied,
            push.rejected.len(),
            push.remaining
        );
        for rejected in &push.rejected {
            println!("  rejected {}：{}", rejected.mid, rejected.error);
        }
    }
    if let Some(error) = &report.push_error {
        println!("push     ! {error}");
    }
    println!(
        "pull     pages={} rows={} removed={} reset={} cursor={}",
        report.pull.pages,
        report.pull.rows,
        report.pull.removed,
        report.pull.reset,
        report.pull.cursor.as_deref().unwrap_or("(none)")
    );
}

/// 入队一条变更，立刻推一次，并展示推送/拉取结果。
fn run_mutation(cli: &Cli, mutation: tela_proto::Mutation, label: &str) {
    let services = real_services(cli, false);
    if !services.is_connected() {
        fail(&tela_proto::TelaError::NotLoggedIn {
            detail: "推送变更需要登录".to_string(),
        });
    }
    match services.enqueue_mutation(&mutation) {
        Ok(true) => println!("已入队 {label}（mid={}）", mutation.mid()),
        Ok(false) => println!("{label} 已在队列中（mid={}），不重复入队", mutation.mid()),
        Err(error) => fail(&error),
    }
    let opts = PullOptions::new(lang_of(cli));
    match services.sync_with(&opts) {
        Ok(report) => print_sync_report(&report),
        Err(error) => {
            eprintln!("tela-app: 推送/同步失败：{error}");
            std::process::exit(error.exit_code());
        }
    }
}

fn run_list(cli: &Cli, feed: Option<i64>, unread_only: bool) {
    let services = real_services(cli, false);
    if let Some(feed_id) = feed {
        let articles = match services.list_articles(feed_id) {
            Ok(articles) => articles,
            Err(error) => fail(&error),
        };
        if articles.is_empty() {
            println!("feed {feed_id} 没有本地文章；先运行 `tela-app sync`。");
            return;
        }
        for article in articles {
            if unread_only && !article.unread {
                continue;
            }
            println!(
                "{} {:>6}  {}  {}",
                if article.unread { "●" } else { " " },
                article.id,
                format_time(article.published_at),
                article.title
            );
        }
        return;
    }

    let feeds = match services.list_feeds() {
        Ok(feeds) => feeds,
        Err(error) => fail(&error),
    };
    if feeds.is_empty() {
        println!("还没有订阅；同步后仍为空就说明账号里没有订阅。");
        return;
    }
    for feed in feeds {
        println!(
            "{:>6}  {:>3}/{:<3}  {:<28}  [{}]",
            feed.id, feed.unread, feed.article_count, feed.title, feed.status
        );
    }
}

fn run_read(cli: &Cli, mode: ViewMode, id: i64, translate: bool, no_mark_read: bool) {
    let services = real_services(cli, false);
    if translate && !services.is_connected() {
        fail(&tela_proto::TelaError::NotLoggedIn {
            detail: "请求翻译需要登录".to_string(),
        });
    }
    if translate {
        match services.request_translation(id, Some(&lang_of(cli))) {
            Ok(answer) => {
                println!("翻译请求：{:?}", answer.status);
                if let Some(reason) = answer.reason {
                    println!("  原因：{reason}");
                }
                if let Some(resets_at) = answer.resets_at {
                    println!("  额度重置：{}", format_time(Some(resets_at)));
                }
            }
            Err(error) => eprintln!("翻译请求失败：{error}"),
        }
    }

    let request = match services.chapter_request_for(id) {
        Ok(request) => request,
        Err(error) => fail(&error),
    };
    let loaded = services.load_chapter(id);

    // 打开即已读：正文成功取到后才写，失败页不算读过。
    if loaded.is_ok() && !no_mark_read {
        match services.mark_read(id) {
            Ok(true) => {
                if let Err(error) = services.sync_now() {
                    eprintln!("tela-app: 已读状态已入队，但同步失败：{error}");
                }
            }
            Ok(false) => {}
            Err(error) => eprintln!("tela-app: 标记已读失败：{error}"),
        }
    }

    let mut session = ReaderSession::new(services.content(), request).with_page_size(cli.page_size);
    session.set_mode(mode);
    session.apply_load_result(loaded);
    render_and_interact(session, Some((id, &services)));
}

fn run_meta(_cli: &Cli) {
    // `/meta` 不需要令牌，也不该因为凭据库不可用而失败。
    let base = std::env::var(tela_app::services::ENV_API_BASE_URL)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| tela_proto::DEFAULT_API_BASE_URL.to_string());
    let client = match tela_net::ApiClient::public(base) {
        Ok(client) => client,
        Err(error) => fail(&error),
    };
    match client.meta() {
        Ok(meta) => match serde_json::to_string_pretty(&meta) {
            Ok(json) => println!("{json}"),
            Err(error) => fail(&tela_proto::TelaError::Config {
                detail: format!("meta 无法序列化：{error}"),
            }),
        },
        Err(error) => fail(&error),
    }
}

/// 渲染并进入交互循环；`retry` 为 `Some` 时 `r` 走带缓存的取数。
fn render_and_interact(mut session: ReaderSession, retry: Option<(i64, &AppServices)>) {
    let mut stdout = io::stdout();
    print_page(&mut stdout, &session);
    if session.state().is_loading() {
        session.load();
    }
    log_load_failure(&session);
    print_page(&mut stdout, &session);

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let mut changed = true;
        match line.trim().to_ascii_lowercase().as_str() {
            "q" | "quit" | "exit" => break,
            "n" | "next" => {
                session.next_page();
            }
            "p" | "prev" => changed = session.prev_page(),
            "r" | "retry" => {
                session.mark_retry();
                match retry {
                    Some((id, services)) => session.apply_load_result(services.load_chapter(id)),
                    None => session.retry(),
                }
            }
            "o" | "original" => session.set_mode(ViewMode::Original),
            "c" | "compare" => session.set_mode(ViewMode::Compare),
            "t" | "translation" => session.set_mode(ViewMode::Translation),
            _ => changed = false,
        }
        if changed {
            print_page(&mut stdout, &session);
        }
    }
}

fn print_page(stdout: &mut io::Stdout, session: &ReaderSession) {
    let mut text = String::new();
    for line in render_session(session) {
        text.push_str(&line);
        text.push('\n');
    }
    text.push('\n');
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

fn log_load_failure(session: &ReaderSession) {
    let Some(error) = session.state().error_message() else {
        return;
    };
    eprintln!("tela-app: 内容加载失败：{error}");
}

fn fail(error: &tela_proto::TelaError) -> ! {
    eprintln!("tela-app: {error}");
    std::process::exit(error.exit_code());
}

fn format_time(ms: Option<i64>) -> String {
    let Some(ms) = ms else {
        return "          ".to_string();
    };
    // 只显示 UTC 日期，避免引入时区依赖；阅读顺序已经由 sortAt 决定。
    let days = ms.div_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant 的 civil_from_days 算法（1970-01-01 起的天数 → 年月日）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + i64::from(m <= 2), m as u32, d as u32)
}
