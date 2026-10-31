//! GUI 宿主与视图层（`gui` feature）。
//!
//! 这一层只做两件事：
//! 1. 启动 GPUI（[`run_demo`]），把窗口生命周期收在一个入口里；
//! 2. 用 GPUI 元素把 [`crate::shell`] 的纯视图模型画出来。
//!
//! 业务取数、SQLite、同步都不在这里：调用方（`tela-app`）负责装配，
//! 再把只读快照交给视图。这样无显示器环境仍然可以测纯逻辑（见 `tests/`），
//! GUI 层只承担“翻译成元素”的职责。

use std::sync::Arc;

use tela_content::api::ContentApi;

use crate::account::AccountService;
use crate::shell::{OfflineCacheService, RecommendationService, ShellData};

use gpui_kit::base::v_flex;
use gpui_kit::*;

pub mod reader;
pub mod shell;
pub use reader::ReaderView;
pub use shell::ShellView;

/// 应用窗口标题（同时用作测试断言的可读标识）。
pub const WINDOW_TITLE: &str = "Tela Desktop";

/// GUI 启动参数：数据源 + 内容 API。
///
/// 两者分开传，是因为订阅/文章来自本地 `Store`，正文来自 `ContentApi`；
/// GUI 层不关心它们由 `tela-app` 的哪个对象实现。
pub struct GuiApp {
    data: Arc<dyn ShellData>,
    content: Arc<dyn ContentApi>,
    accounts: Option<Arc<dyn AccountService>>,
    offline_cache: Option<Arc<dyn OfflineCacheService>>,
    recommendations: Option<Arc<dyn RecommendationService>>,
}

impl GuiApp {
    /// 装配 GUI 启动参数（不带账号能力；demo/测试）。
    pub fn new(data: Arc<dyn ShellData>, content: Arc<dyn ContentApi>) -> Self {
        Self {
            data,
            content,
            accounts: None,
            offline_cache: None,
            recommendations: None,
        }
    }

    /// 带 Login/Logout 能力的装配（真实 GUI）。
    pub fn with_accounts(
        data: Arc<dyn ShellData>,
        content: Arc<dyn ContentApi>,
        accounts: Arc<dyn AccountService>,
    ) -> Self {
        Self {
            data,
            content,
            accounts: Some(accounts),
            offline_cache: None,
            recommendations: None,
        }
    }

    /// 装配订阅级离线缓存能力（左侧博客列表的“缓存”按钮）。
    pub fn with_offline_cache(mut self, cache: Arc<dyn OfflineCacheService>) -> Self {
        self.offline_cache = Some(cache);
        self
    }

    /// 装配推荐写能力（阅读面板的“推荐”按钮与“我的推荐”列表）。
    pub fn with_recommendations(mut self, recommendations: Arc<dyn RecommendationService>) -> Self {
        self.recommendations = Some(recommendations);
        self
    }
}

/// S0 启动冒烟视图：只证明「GPUI → 窗口 → 绘制 → 测试 harness」整条链路可用。
///
/// S2 起由 [`crate::gui::shell::ShellView`] 取代；保留它作为无业务依赖的最小回归点，
/// 便于把「窗口起不来」与「业务视图画错」两类故障分开定位。
pub struct BootView {
    title: SharedString,
}

impl BootView {
    /// 构造一个最小视图。
    pub fn new() -> Self {
        Self {
            title: WINDOW_TITLE.into(),
        }
    }

    /// 视图标题（测试断言用，避免依赖像素或可访问性树）。
    pub fn title(&self) -> &str {
        &self.title
    }
}

impl Default for BootView {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for BootView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(rgb(0x11111b))
            .text_color(rgb(0xcdd6f4))
            .p_8()
            .gap_3()
            .child(
                div()
                    .id("app-title")
                    .aria_label(WINDOW_TITLE)
                    .test_support()
                    .text_2xl()
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .id("boot-status")
                    .aria_label("GPUI host ready")
                    .test_support()
                    .text_sm()
                    .text_color(rgb(0xa6adc8))
                    .child("GPUI 宿主已启动"),
            )
    }
}

/// 打开一个标准 Tela 窗口。
fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1100.), px(720.)),
            cx,
        ))),
        window_min_size: Some(size(px(880.), px(560.))),
        app_id: Some("tela-desktop".to_string()),
        titlebar: Some(TitlebarOptions {
            title: Some(WINDOW_TITLE.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// S2 启动入口：打开三栏外壳窗口。
///
/// 关闭最后一个窗口即退出应用；这是桌面应用的标准生命周期，
/// 避免“窗口关了但进程还挂在后台”。
pub fn run(app: GuiApp) {
    let data = app.data;
    let content = app.content;
    let accounts = app.accounts;
    let offline_cache = app.offline_cache;
    let recommendations = app.recommendations;
    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        gpui_kit::open_window(window_options(cx), cx, move |_, cx| {
            cx.new(|cx| {
                ShellView::new_with_recommendations(
                    data.clone(),
                    content.clone(),
                    accounts.clone(),
                    offline_cache.clone(),
                    recommendations.clone(),
                    cx,
                )
            })
        })
        .expect("open Tela window");
        cx.activate(true);
    });
}

/// S0 启动入口：打开窗口并显示 [`BootView`]。
///
/// 保留它作为无业务依赖的最小回归点，便于把「窗口起不来」与
/// 「业务视图画错」两类故障分开定位。
pub fn run_demo() {
    gpui_kit::application().run(|cx: &mut App| {
        gpui_kit::init(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        gpui_kit::open_window(window_options(cx), cx, |_, cx| cx.new(|_| BootView::new()))
            .expect("open Tela window");
        cx.activate(true);
    });
}
