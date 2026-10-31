//! S2 三栏外壳的窗口级测试：真实 GPUI 窗口 + 纯数据源，验证渲染与选中切换。
//!
//! 元素级文案/点击由 `shell_view_model` 的纯模型测试覆盖；这里只保证
//! “视图能装配、能渲染、切换订阅后模型与帧都更新”，不依赖像素或显示器。

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    AppContext, Bounds, Entity, Point, Render, TestAppContext, Window, WindowBounds, WindowOptions,
    px, size,
};
use tela_content::api::{ChapterRequest, ContentApi};
use tela_content::mock::{MOCK_CONTENT_KEY, MockContentApi};
use tela_proto::TelaError;
use tela_ui::gui::ShellView;
use tela_ui::shell::{ArticleItem, FeedItem, ShellData};

#[derive(Default)]
struct FakeData {
    feeds: Vec<FeedItem>,
    articles: HashMap<i64, Vec<ArticleItem>>,
    /// 指定所有文章共用的正文 key；默认用 article id（多数测试只验证列表）。
    content_key: Option<String>,
}

impl ShellData for FakeData {
    fn feeds(&self) -> Result<Vec<FeedItem>, TelaError> {
        Ok(self.feeds.clone())
    }

    fn articles(&self, feed_id: i64) -> Result<Vec<ArticleItem>, TelaError> {
        Ok(self.articles.get(&feed_id).cloned().unwrap_or_default())
    }

    fn chapter_request(&self, article_id: i64) -> Result<ChapterRequest, TelaError> {
        Ok(ChapterRequest::new(
            self.content_key
                .clone()
                .unwrap_or_else(|| article_id.to_string()),
            "zh-Hans",
        ))
    }
}

fn feed(id: i64, title: &str, unread: usize, count: usize) -> FeedItem {
    FeedItem {
        id,
        title: title.to_string(),
        feed_url: format!("https://example.com/{id}.xml"),
        unread,
        article_count: count,
        status: "active".to_string(),
    }
}

fn article(id: i64, feed_id: i64, title: &str) -> ArticleItem {
    ArticleItem {
        id,
        feed_id,
        url: Some(format!("https://example.com/articles/{id}")),
        title: title.to_string(),
        unread: true,
        published_at: Some(1_761_350_400 + id),
        excerpt: Some(format!("{title} 的摘要")),
    }
}

fn open_window<V: Render>(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut Window, &mut gpui::App) -> Entity<V>,
) -> (gpui::AnyWindowHandle, Entity<V>) {
    open_window_sized(cx, size(px(1100.), px(720.)), build)
}

fn open_window_sized<V: Render>(
    cx: &mut TestAppContext,
    window_size: gpui::Size<gpui::Pixels>,
    build: impl FnOnce(&mut Window, &mut gpui::App) -> Entity<V>,
) -> (gpui::AnyWindowHandle, Entity<V>) {
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: window_size,
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
fn shell_renders_and_switches_feeds(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let data = Arc::new(FakeData {
        feeds: vec![feed(1, "Tela 示例博客", 2, 2), feed(2, "技术随笔", 1, 1)],
        articles: HashMap::from([
            (
                1,
                vec![article(42, 1, "Hello Tela"), article(43, 1, "长章节")],
            ),
            (2, vec![article(45, 2, "Rust 周报")]),
        ]),
        ..FakeData::default()
    });
    let data_for_view: Arc<dyn ShellData> = data;
    let (window, view) = open_window(cx, move |_, cx| {
        cx.new(|cx| ShellView::new(data_for_view, content(), cx))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        let model = view.read(cx).model();
        assert_eq!(model.feeds.len(), 2);
        assert_eq!(model.selected_feed, Some(1));
        assert_eq!(model.selected_article, Some(42));
        assert_eq!(model.unread_total(), 3);

        // 切换订阅必须重载文章并重置文章选中，且下一帧仍能画出来。
        let changed = view.update(cx, |view, cx| {
            let changed = view.select_feed(2, cx);
            cx.notify();
            changed
        });
        assert!(changed);
        window.draw(cx).clear(cx);
        let model = view.read(cx).model();
        assert_eq!(model.selected_feed, Some(2));
        assert_eq!(model.selected_article, Some(45));
        assert_eq!(model.articles.len(), 1);
    })
    .unwrap();
}

#[gpui::test]
fn shell_renders_empty_state_without_panicking(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let data: Arc<dyn ShellData> = Arc::new(FakeData::default());
    let (window, view) = open_window(cx, move |_, cx| {
        cx.new(|cx| ShellView::new(data, content(), cx))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        let model = view.read(cx).model();
        assert!(model.is_empty());
        assert!(model.feeds_error.is_none());
        assert_eq!(model.status_line(), "还没有订阅");
    })
    .unwrap();
}

/// 键盘快捷键：j/k 文章，Shift+j/k 博客；事件从阅读面板冒泡到外壳。
#[gpui::test]
fn keyboard_navigates_articles_and_feeds(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let data = Arc::new(FakeData {
        feeds: vec![feed(1, "A", 3, 3), feed(2, "B", 2, 2)],
        articles: HashMap::from([
            (
                1,
                vec![
                    article(10, 1, "A1"),
                    article(11, 1, "A2"),
                    article(12, 1, "A3"),
                ],
            ),
            (2, vec![article(20, 2, "B1"), article(21, 2, "B2")]),
        ]),
        content_key: Some(MOCK_CONTENT_KEY.to_string()),
    });
    let data_for_view: Arc<dyn ShellData> = data;
    let (window, view) = open_window_sized(cx, size(px(900.), px(420.)), move |_, cx| {
        cx.new(|cx| ShellView::new(data_for_view, content(), cx))
    });

    // 首帧让阅读面板拿到焦点（它会在 render 里 focus 自己），再画一帧
    // 让异步加载的图片进入布局，正文才会真的超过一屏。
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    assert_eq!(
        cx.update(|cx| view.read(cx).reader().read(cx).session().page()),
        0
    );

    // PageDown/PageUp 必须穿过可选择的 TextView，按视口滚动正文。
    // 这篇 5 段正文比窗口高，但段落分页只有 1 页。
    let before = cx.update(|cx| {
        let reader = view.read(cx).reader();
        let reader = reader.read(cx);
        (reader.session().page(), reader.scroll_offset().y)
    });
    assert_eq!(before, (0, px(0.)));

    cx.simulate_keystrokes(window, "pagedown");
    let after_down = cx.update(|cx| {
        let reader = view.read(cx).reader();
        let reader = reader.read(cx);
        (reader.session().page(), reader.scroll_offset().y)
    });
    assert_eq!(after_down.0, 0, "不应再按段落切页");
    assert!(after_down.1 < px(0.), "PageDown 应向下滚动正文");

    cx.simulate_keystrokes(window, "pageup");
    let after_up = cx.update(|cx| {
        let reader = view.read(cx).reader();
        let reader = reader.read(cx);
        reader.scroll_offset().y
    });
    assert_eq!(after_up, px(0.), "PageUp 应回到顶部");

    // ↑/↓ 每次只滚一行，且到顶后停住。
    let line = cx.update(|cx| {
        let reader = view.read(cx).reader();
        reader.read(cx).line_height()
    });
    cx.simulate_keystrokes(window, "down");
    let one_line = cx.update(|cx| {
        let reader = view.read(cx).reader();
        reader.read(cx).scroll_offset().y
    });
    assert_eq!(one_line, px(0.) - line, "↓ 应只滚动一行");
    cx.simulate_keystrokes(window, "up");
    let back_to_top = cx.update(|cx| {
        let reader = view.read(cx).reader();
        reader.read(cx).scroll_offset().y
    });
    assert_eq!(back_to_top, px(0.), "↑ 应回到顶部");
    cx.simulate_keystrokes(window, "up");
    let still_top = cx.update(|cx| {
        let reader = view.read(cx).reader();
        reader.read(cx).scroll_offset().y
    });
    assert_eq!(still_top, px(0.), "已经在顶部时 ↑ 不应继续滚动");
    assert_eq!(
        cx.update(|cx| view.read(cx).reader().read(cx).session().page()),
        0,
        "逐行滚动不应切段落页"
    );

    cx.simulate_keystrokes(window, "j");
    assert_eq!(
        cx.update(|cx| view.read(cx).model().selected_article),
        Some(11)
    );
    cx.simulate_keystrokes(window, "j");
    assert_eq!(
        cx.update(|cx| view.read(cx).model().selected_article),
        Some(12)
    );
    cx.simulate_keystrokes(window, "j");
    assert_eq!(
        cx.update(|cx| view.read(cx).model().selected_article),
        Some(12),
        "最后一篇后停住"
    );
    cx.simulate_keystrokes(window, "k");
    assert_eq!(
        cx.update(|cx| view.read(cx).model().selected_article),
        Some(11)
    );

    cx.simulate_keystrokes(window, "shift-j");
    assert_eq!(cx.update(|cx| view.read(cx).model().selected_feed), Some(2));
    assert_eq!(
        cx.update(|cx| view.read(cx).model().selected_article),
        Some(20),
        "换博客后打开第一篇"
    );
    cx.simulate_keystrokes(window, "shift-k");
    assert_eq!(cx.update(|cx| view.read(cx).model().selected_feed), Some(1));
    assert_eq!(
        cx.update(|cx| view.read(cx).model().selected_article),
        Some(10)
    );

    // 下一帧仍然能渲染（快捷键没有把视图卡在错误状态）。
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
}
