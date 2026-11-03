//! 主题窗口级测试：内置主题都能画、切换后全局主题与 `ui.toml` 都更新。
//!
//! 纯逻辑（解析/校验/持久化）在 `tela-ui::theme` 的单元测试里；这里只验证
//! 「视图能切主题、切换后下一帧仍能渲染」这条 GUI 链路。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    AppContext, Bounds, Entity, Keystroke, Point, Render, TestAppContext, Window, WindowBounds,
    WindowOptions, px, size,
};
use tela_content::api::{ChapterRequest, ContentApi};
use tela_content::mock::{MOCK_CONTENT_KEY, MockContentApi};
use tela_proto::TelaError;
use tela_ui::gui::ShellView;
use tela_ui::gui::theme::ActiveTelaTheme as _;
use tela_ui::reader::ViewMode;
use tela_ui::shell::{ArticleItem, FeedItem, ShellData};
use tela_ui::theme::{ThemeCatalog, ThemePaths};

struct EmptyData;

impl ShellData for EmptyData {
    fn feeds(&self) -> Result<Vec<FeedItem>, TelaError> {
        Ok(Vec::new())
    }

    fn articles(&self, _feed_id: i64) -> Result<Vec<ArticleItem>, TelaError> {
        Ok(Vec::new())
    }

    fn chapter_request(&self, _article_id: i64) -> Result<ChapterRequest, TelaError> {
        Ok(ChapterRequest::new(MOCK_CONTENT_KEY, "zh-Hans"))
    }
}

struct OneArticleData;

impl ShellData for OneArticleData {
    fn feeds(&self) -> Result<Vec<FeedItem>, TelaError> {
        Ok(vec![FeedItem {
            id: 1,
            title: "Tela 示例博客".to_string(),
            feed_url: "https://example.com/tela.xml".to_string(),
            unread: 1,
            article_count: 1,
            status: "active".to_string(),
        }])
    }

    fn articles(&self, _feed_id: i64) -> Result<Vec<ArticleItem>, TelaError> {
        Ok(vec![ArticleItem {
            id: 42,
            feed_id: 1,
            url: Some("https://example.com/articles/42".to_string()),
            title: "Hello Tela".to_string(),
            unread: true,
            published_at: Some(1_761_350_700),
            excerpt: Some("一段摘要".to_string()),
        }])
    }

    fn chapter_request(&self, _article_id: i64) -> Result<ChapterRequest, TelaError> {
        Ok(ChapterRequest::new(MOCK_CONTENT_KEY, "zh-Hans"))
    }
}

fn content() -> Arc<dyn ContentApi> {
    Arc::new(MockContentApi::new())
}

fn temp_dir(tag: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "tela-gui-theme-{tag}-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
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

fn draw_once(cx: &mut TestAppContext, window: gpui::AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
}

#[gpui::test]
fn theme_picker_applies_and_persists(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = temp_dir("apply");
    let paths = ThemePaths::from_data_dir(&dir);
    let catalog = ThemeCatalog::load(&paths);
    let data: Arc<dyn ShellData> = Arc::new(EmptyData);
    let (window, view) = open_window(cx, move |_, cx| {
        cx.new(|cx| {
            ShellView::new_with_theme_catalog(data, content(), None, None, None, None, catalog, cx)
        })
    });
    draw_once(cx, window);

    cx.update_window(window, |_, window, cx| {
        let view_ref = &view;
        assert_eq!(view_ref.read(cx).active_theme_id(), "catppuccin-mocha");
        assert_eq!(cx.tela_theme().id, "catppuccin-mocha");

        view.update(cx, |view, cx| view.open_theme_picker(cx));
        assert!(view.read(cx).theme_picker_open());
        window.draw(cx).clear(cx);

        let applied = view.update(cx, |view, cx| view.apply_theme("catppuccin-latte", cx));
        assert!(applied);
        assert_eq!(view.read(cx).active_theme_id(), "catppuccin-latte");
        assert_eq!(cx.tela_theme().id, "catppuccin-latte");
        assert!(cx.tela_theme().appearance.is_light());
        // 选择要立刻落盘，重启后首帧就是浅色主题。
        let saved = std::fs::read_to_string(paths.settings_path.as_ref().unwrap()).unwrap();
        assert!(saved.contains("catppuccin-latte"));

        // 未知主题不改变选择，只给提示。
        let missing = view.update(cx, |view, cx| view.apply_theme("missing", cx));
        assert!(!missing);
        assert_eq!(cx.tela_theme().id, "catppuccin-latte");

        view.update(cx, |view, cx| view.close_theme_picker(cx));
        assert!(!view.read(cx).theme_picker_open());
        window.draw(cx).clear(cx);
    })
    .unwrap();

    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn every_builtin_theme_renders(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let catalog = ThemeCatalog::builtin();
    let ids: Vec<String> = catalog
        .themes()
        .iter()
        .map(|theme| theme.id.clone())
        .collect();
    assert!(ids.len() >= 3);

    let data: Arc<dyn ShellData> = Arc::new(EmptyData);
    let (window, view) = open_window(cx, move |_, cx| {
        cx.new(|cx| {
            ShellView::new_with_theme_catalog(data, content(), None, None, None, None, catalog, cx)
        })
    });
    draw_once(cx, window);

    cx.update_window(window, |_, window, cx| {
        for id in ids {
            let applied = view.update(cx, |view, cx| view.apply_theme(&id, cx));
            assert!(applied, "theme {id} should be selectable");
            // 切换后至少画一帧：字体、图标、边框都来自新主题。
            window.draw(cx).clear(cx);
        }
    })
    .unwrap();
}

/// 主题选择器是窗口级模态：遮罩后面阅读面板的 `o/c/t` 不能生效。
#[gpui::test]
fn theme_picker_locks_reader_shortcuts(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let data: Arc<dyn ShellData> = Arc::new(OneArticleData);
    let catalog = ThemeCatalog::builtin();
    let (window, view) = open_window(cx, move |_, cx| {
        cx.new(|cx| {
            ShellView::new_with_theme_catalog(data, content(), None, None, None, None, catalog, cx)
        })
    });
    draw_once(cx, window);
    cx.run_until_parked();
    draw_once(cx, window);

    let reader = view.read_with(cx, |view, _| view.reader());
    assert_eq!(
        reader.read_with(cx, |reader, _| reader.session().mode()),
        ViewMode::Compare
    );

    cx.update_window(window, |_, window, cx| {
        view.update(cx, |view, cx| view.open_theme_picker(cx));
        window.dispatch_keystroke(Keystroke::parse("t").expect("keystroke"), cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    // 遮罩打开时按 t：没有切换视图。
    assert_eq!(
        reader.read_with(cx, |reader, _| reader.session().mode()),
        ViewMode::Compare
    );

    cx.update_window(window, |_, window, cx| {
        view.update(cx, |view, cx| view.close_theme_picker(cx));
        window.dispatch_keystroke(Keystroke::parse("t").expect("keystroke"), cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    // 关闭后恢复快捷键。
    assert_eq!(
        reader.read_with(cx, |reader, _| reader.session().mode()),
        ViewMode::Translation
    );
}
