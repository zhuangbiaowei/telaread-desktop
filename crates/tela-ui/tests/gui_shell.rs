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
use tela_content::mock::MockContentApi;
use tela_proto::TelaError;
use tela_ui::gui::ShellView;
use tela_ui::shell::{ArticleItem, FeedItem, ShellData};

#[derive(Default)]
struct FakeData {
    feeds: Vec<FeedItem>,
    articles: HashMap<i64, Vec<ArticleItem>>,
}

impl ShellData for FakeData {
    fn feeds(&self) -> Result<Vec<FeedItem>, TelaError> {
        Ok(self.feeds.clone())
    }

    fn articles(&self, feed_id: i64) -> Result<Vec<ArticleItem>, TelaError> {
        Ok(self.articles.get(&feed_id).cloned().unwrap_or_default())
    }

    fn chapter_request(&self, article_id: i64) -> Result<ChapterRequest, TelaError> {
        Ok(ChapterRequest::new(article_id.to_string(), "zh-Hans"))
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
