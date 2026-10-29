//! GUI 账号面板：状态展示、登录状态机、退出登录。
//!
//! 用 fake `AccountService`，不联网、不碰 Keyring；只验证 `ShellView` 对 trait 的调用
//! 与状态迁移，因此可在无显示器环境运行。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{
    AppContext, Bounds, Entity, Point, Render, TestAppContext, Window, WindowBounds, WindowOptions,
    px, size,
};
use tela_content::api::{ChapterRequest, ContentApi};
use tela_content::mock::MockContentApi;
use tela_proto::TelaError;
use tela_ui::account::{AccountMode, AccountService, AccountStatus, LoginPhase, LoginTicket};
use tela_ui::gui::ShellView;
use tela_ui::shell::{ArticleItem, FeedItem, ShellData};

struct FakeData;

impl ShellData for FakeData {
    fn feeds(&self) -> Result<Vec<FeedItem>, TelaError> {
        Ok(vec![FeedItem {
            id: 1,
            title: "Tela 示例博客".into(),
            feed_url: "https://example.com/feed.xml".into(),
            unread: 2,
            article_count: 2,
            status: "active".into(),
        }])
    }

    fn articles(&self, _feed_id: i64) -> Result<Vec<ArticleItem>, TelaError> {
        Ok(vec![
            ArticleItem {
                id: 42,
                feed_id: 1,
                title: "Hello Tela".into(),
                unread: true,
                published_at: Some(1_761_350_400_000),
                excerpt: None,
            },
            ArticleItem {
                id: 43,
                feed_id: 1,
                title: "第二篇".into(),
                unread: true,
                published_at: Some(1_761_350_300_000),
                excerpt: None,
            },
        ])
    }

    fn chapter_request(&self, article_id: i64) -> Result<ChapterRequest, TelaError> {
        Ok(ChapterRequest::new(article_id.to_string(), "zh-Hans"))
    }
}

struct FakeAccounts {
    status: Mutex<AccountStatus>,
    begin_calls: AtomicUsize,
    open_calls: AtomicUsize,
    sync_calls: AtomicUsize,
    cancel_calls: AtomicUsize,
    mark_read_calls: AtomicUsize,
}

impl FakeAccounts {
    fn new(connected: bool) -> Self {
        Self {
            status: Mutex::new(AccountStatus {
                mode: AccountMode::Real,
                connected,
                handle: if connected { Some("ann".into()) } else { None },
                scope: if connected {
                    Some("write".into())
                } else {
                    None
                },
                client_id_configured: true,
                store_error: None,
                db_path: "/tmp/tela.db".into(),
            }),
            begin_calls: AtomicUsize::new(0),
            open_calls: AtomicUsize::new(0),
            sync_calls: AtomicUsize::new(0),
            cancel_calls: AtomicUsize::new(0),
            mark_read_calls: AtomicUsize::new(0),
        }
    }

    fn set(&self, connected: bool) {
        let mut status = self.status.lock().unwrap();
        status.connected = connected;
        status.handle = connected.then(|| "ann".to_string());
        status.scope = connected.then(|| "write".to_string());
    }
}

impl AccountService for FakeAccounts {
    fn status(&self) -> AccountStatus {
        self.status.lock().unwrap().clone()
    }

    fn begin_login(&self) -> Result<LoginTicket, TelaError> {
        self.begin_calls.fetch_add(1, Ordering::SeqCst);
        Ok(LoginTicket {
            nonce: 7,
            auth_url: "https://telaread.com/connect?client_id=test".into(),
        })
    }

    fn wait_login(&self, _ticket: &LoginTicket) -> Result<AccountStatus, TelaError> {
        self.set(true);
        Ok(self.status())
    }

    fn open_auth_url(&self, _ticket: &LoginTicket) -> Result<(), TelaError> {
        self.open_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn cancel_login(&self) {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
    }

    fn logout(&self) -> Result<AccountStatus, TelaError> {
        self.set(false);
        Ok(self.status())
    }

    fn sync_now(&self) -> Result<(), TelaError> {
        self.sync_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn mark_read(&self, _article_id: i64) -> Result<bool, TelaError> {
        self.mark_read_calls.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
}

fn open_window<V: Render>(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut Window, &mut gpui::App) -> Entity<V>,
) -> (gpui::AnyWindowHandle, Entity<V>) {
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1100.), px(720.)),
                })),
                ..Default::default()
            },
            cx,
            build,
        )
        .expect("open test window")
    })
}

fn content() -> Arc<dyn ContentApi> {
    Arc::new(MockContentApi::new())
}

#[gpui::test]
fn account_panel_shows_connected_state_without_panicking(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let accounts = Arc::new(FakeAccounts::new(true));
    let (window, view) = open_window(cx, move |_, cx| {
        cx.new(|cx| ShellView::new_with_accounts(Arc::new(FakeData), content(), Some(accounts), cx))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        let view = view.read(cx);
        assert!(view.account().connected);
        assert_eq!(view.account().handle.as_deref(), Some("ann"));
        assert!(view.account().can_logout());
        assert!(!view.account().can_login());
    })
    .unwrap();
}

#[gpui::test]
fn login_moves_from_idle_to_connected_and_syncs(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let accounts = Arc::new(FakeAccounts::new(false));
    let view = cx.new(|cx| {
        ShellView::new_with_accounts(
            Arc::new(FakeData),
            content(),
            Some(Arc::clone(&accounts) as Arc<dyn AccountService>),
            cx,
        )
    });

    assert!(!view.read_with(cx, |view, _| view.account().connected));
    view.update(cx, |view, cx| view.start_login(cx));
    cx.run_until_parked();

    view.read_with(cx, |view, _| {
        assert!(view.account().connected);
        assert_eq!(view.account().handle.as_deref(), Some("ann"));
        assert!(
            view.login_phase() == &LoginPhase::Idle,
            "{:?}",
            view.login_phase()
        );
    });
    assert_eq!(accounts.begin_calls.load(Ordering::SeqCst), 1);
    assert_eq!(accounts.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(accounts.sync_calls.load(Ordering::SeqCst), 1);
}

#[gpui::test]
fn logout_returns_to_disconnected_state(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let accounts = Arc::new(FakeAccounts::new(true));
    let view = cx.new(|cx| {
        ShellView::new_with_accounts(
            Arc::new(FakeData),
            content(),
            Some(Arc::clone(&accounts) as Arc<dyn AccountService>),
            cx,
        )
    });

    assert!(view.read_with(cx, |view, _| view.account().connected));
    view.update(cx, |view, cx| view.logout(cx));
    cx.run_until_parked();

    view.read_with(cx, |view, _| {
        assert!(!view.account().connected);
        assert!(view.account().can_login());
    });
}

#[gpui::test]
fn selecting_an_article_marks_it_read_and_schedules_a_sync(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let accounts = Arc::new(FakeAccounts::new(true));
    let view = cx.new(|cx| {
        ShellView::new_with_accounts(
            Arc::new(FakeData),
            content(),
            Some(Arc::clone(&accounts) as Arc<dyn AccountService>),
            cx,
        )
    });

    let changed = view.update(cx, |view, cx| view.select_article(43, cx));
    assert!(changed);
    assert_eq!(accounts.mark_read_calls.load(Ordering::SeqCst), 1);
    cx.run_until_parked();
    assert_eq!(
        accounts.sync_calls.load(Ordering::SeqCst),
        1,
        "本地标记后必须在后台推一次"
    );
}

#[gpui::test]
fn clicking_the_already_open_article_still_marks_it_read(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let accounts = Arc::new(FakeAccounts::new(true));
    let view = cx.new(|cx| {
        ShellView::new_with_accounts(
            Arc::new(FakeData),
            content(),
            Some(Arc::clone(&accounts) as Arc<dyn AccountService>),
            cx,
        )
    });

    // 启动时自动展示第一篇（42），但没有自动标已读；用户点它一次就应该标。
    assert_eq!(
        view.read_with(cx, |view, _| view.model().selected_article),
        Some(42)
    );
    let handled = view.update(cx, |view, cx| view.select_article(42, cx));
    assert!(handled);
    assert_eq!(accounts.mark_read_calls.load(Ordering::SeqCst), 1);
}
