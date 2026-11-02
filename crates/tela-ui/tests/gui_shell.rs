//! S2 三栏外壳的窗口级测试：真实 GPUI 窗口 + 纯数据源，验证渲染与选中切换。
//!
//! 元素级文案/点击由 `shell_view_model` 的纯模型测试覆盖；这里只保证
//! “视图能装配、能渲染、切换订阅后模型与帧都更新”，不依赖像素或显示器。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gpui::{
    AppContext, Bounds, Entity, Point, Render, TestAppContext, Window, WindowBounds, WindowOptions,
    px, size,
};
use tela_content::api::{ChapterRequest, ContentApi};
use tela_content::mock::{MOCK_CONTENT_KEY, MOCK_LONG_CONTENT_KEY, MockContentApi};
use tela_proto::TelaError;
use tela_ui::gui::ShellView;
use tela_ui::shell::{
    ArticleItem, BookmarkItem, BookmarkService, FeedItem, OfflineCacheProgress, OfflineCacheReport,
    OfflineCacheService, RecommendationItem, RecommendationService, ShellData, ShellSection,
};

#[derive(Default)]
struct FakeData {
    feeds: Vec<FeedItem>,
    articles: HashMap<i64, Vec<ArticleItem>>,
    /// 指定所有文章共用的正文 key；默认用 article id（多数测试只验证列表）。
    content_key: Option<String>,
    /// 与 `FakeRecommender` 共享的推荐列表，模拟本地库的实时变化。
    recommendations: Option<Arc<Mutex<Vec<RecommendationItem>>>>,
    /// 与 `FakeBookmarks` 共享的收藏列表，模拟本地库的实时变化。
    bookmarks: Option<Arc<Mutex<Vec<BookmarkItem>>>>,
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

    fn recommendations(&self) -> Result<Vec<RecommendationItem>, TelaError> {
        Ok(self
            .recommendations
            .as_ref()
            .map(|items| items.lock().map(|items| items.clone()).unwrap_or_default())
            .unwrap_or_default())
    }

    fn bookmarks(&self) -> Result<Vec<BookmarkItem>, TelaError> {
        Ok(self
            .bookmarks
            .as_ref()
            .map(|items| items.lock().map(|items| items.clone()).unwrap_or_default())
            .unwrap_or_default())
    }

    fn bookmark_request(&self, _bookmark_id: i64) -> Result<ChapterRequest, TelaError> {
        // 收藏正文统一用 mock 正文：这里只验证「收藏 → 阅读面板」这条链路。
        Ok(ChapterRequest::new(MOCK_CONTENT_KEY, "zh-Hans"))
    }
}

/// 推荐服务的测试替身：记录调用并同步修改共享的推荐列表。
type RecommendationCalls = Arc<Mutex<Vec<(i64, Option<String>)>>>;

struct FakeRecommender {
    items: Arc<Mutex<Vec<RecommendationItem>>>,
    calls: RecommendationCalls,
}

impl RecommendationService for FakeRecommender {
    fn recommend(&self, article_id: i64, note: Option<String>) -> Result<(), TelaError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push((article_id, note.clone()));
        }
        if let Ok(mut items) = self.items.lock() {
            if let Some(item) = items.iter_mut().find(|item| item.article_id == article_id) {
                item.note = note;
            } else {
                items.push(RecommendationItem {
                    article_id,
                    title: format!("文章 {article_id}"),
                    url: Some(format!("https://example.com/articles/{article_id}")),
                    feed_id: 1,
                    feed_title: "示例博客".to_string(),
                    note,
                    created_at: 2_000,
                });
            }
        }
        Ok(())
    }

    fn unrecommend(&self, article_id: i64) -> Result<(), TelaError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push((article_id, None));
        }
        if let Ok(mut items) = self.items.lock() {
            items.retain(|item| item.article_id != article_id);
        }
        Ok(())
    }
}

fn recommendation(article_id: i64, title: &str) -> RecommendationItem {
    RecommendationItem {
        article_id,
        title: title.to_string(),
        url: Some(format!("https://example.com/articles/{article_id}")),
        feed_id: 1,
        feed_title: "示例博客".to_string(),
        note: Some("值得一读".to_string()),
        created_at: 2_000,
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

/// 画一帧。键盘只登记“要滚到哪个条目”，列表偏移在 prepaint 里才写入
/// `ScrollHandle`，因此断言滚动位置前必须先画一帧。
fn draw_once(cx: &mut TestAppContext, window: gpui::AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
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
        ..FakeData::default()
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

/// 列表跟随选中项：`j/k` 与 `Shift+j/k` 把选中项移出视口时，
/// 文章列表与左侧订阅列表都必须跟着滚，不能只在数据上切换。
#[gpui::test]
fn keyboard_navigation_scrolls_lists_to_follow_selection(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let feeds: Vec<FeedItem> = (1..=12)
        .map(|id| feed(id, &format!("博客 {id}"), 1, 20))
        .collect();
    let mut articles = HashMap::new();
    for feed_id in 1..=12 {
        articles.insert(
            feed_id,
            (0..20)
                .map(|i| article(feed_id * 100 + i, feed_id, &format!("文章 {feed_id}-{i}")))
                .collect::<Vec<_>>(),
        );
    }
    let data: Arc<dyn ShellData> = Arc::new(FakeData {
        feeds,
        articles,
        content_key: Some(MOCK_CONTENT_KEY.to_string()),
        ..FakeData::default()
    });
    // 窗口矮一些：很快就能把选中项移出视口，覆盖“超出屏幕”的路径。
    let (window, view) = open_window_sized(cx, size(px(900.), px(420.)), move |_, cx| {
        cx.new(|cx| ShellView::new(data, content(), cx))
    });

    // 首帧拿焦点，第二帧让列表完成布局/滚动状态初始化。
    draw_once(cx, window);
    cx.run_until_parked();
    draw_once(cx, window);

    let article_offset =
        |cx: &mut TestAppContext| cx.update(|cx| view.read(cx).article_scroll_offset().y);
    let feed_offset =
        |cx: &mut TestAppContext| cx.update(|cx| view.read(cx).feed_scroll_offset().y);

    assert_eq!(article_offset(cx), px(0.), "首帧文章列表在顶部");

    // 一路 j 到第 13 篇：文章列表必须向下滚。
    for _ in 0..12 {
        cx.simulate_keystrokes(window, "j");
        draw_once(cx, window);
    }
    assert_eq!(
        cx.update(|cx| view.read(cx).model().selected_article),
        Some(112),
        "j 应选中第 13 篇"
    );
    assert!(
        article_offset(cx) < px(0.),
        "选中项滚出屏幕后文章列表应跟随，实际偏移 {:?}",
        article_offset(cx)
    );

    // 再一路 k 回到第 1 篇：列表滚回顶部。
    for _ in 0..12 {
        cx.simulate_keystrokes(window, "k");
        draw_once(cx, window);
    }
    assert_eq!(
        cx.update(|cx| view.read(cx).model().selected_article),
        Some(100)
    );
    assert!(
        article_offset(cx) > px(-16.),
        "回到第一篇后列表应回到顶部，实际偏移 {:?}",
        article_offset(cx)
    );

    // 连续 Shift+j 切换订阅：左侧订阅列表同样要跟随。
    for _ in 0..9 {
        cx.simulate_keystrokes(window, "shift-j");
        draw_once(cx, window);
    }
    assert_eq!(
        cx.update(|cx| view.read(cx).model().selected_feed),
        Some(10)
    );
    assert!(
        feed_offset(cx) < px(0.),
        "切换订阅跨出一屏后订阅列表应跟随，实际偏移 {:?}",
        feed_offset(cx)
    );
    // 换订阅后文章列表回到新订阅的第一篇（不保留上一篇的滚动位置）。
    assert_eq!(
        cx.update(|cx| view.read(cx).model().selected_article),
        Some(1000)
    );
    assert!(
        article_offset(cx) > px(-16.),
        "换订阅后文章列表应回到顶部，实际偏移 {:?}",
        article_offset(cx)
    );
}

/// 长文连续阅读：一直按 PageDown 必须能一路滚到全文末尾，中途不切段落页。
#[gpui::test]
fn page_down_reaches_the_end_of_a_long_article_without_paging(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let data = Arc::new(FakeData {
        feeds: vec![feed(1, "A", 1, 1)],
        articles: HashMap::from([(1, vec![article(43, 1, "长章节")])]),
        content_key: Some(MOCK_LONG_CONTENT_KEY.to_string()),
        ..FakeData::default()
    });
    let data_for_view: Arc<dyn ShellData> = data;
    // 用较大的窗口：阅读区更宽、每段行数更少，测试不必逐屏按上百次。
    let (window, view) = open_window_sized(cx, size(px(1440.), px(900.)), move |_, cx| {
        cx.new(|cx| ShellView::new(data_for_view, content(), cx))
    });

    // 首帧拿焦点，第二帧让整篇正文进入布局。
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();

    let offset =
        |cx: &mut TestAppContext| cx.update(|cx| view.read(cx).reader().read(cx).scroll_offset().y);
    let page =
        |cx: &mut TestAppContext| cx.update(|cx| view.read(cx).reader().read(cx).session().page());
    let rows = |cx: &mut TestAppContext| {
        cx.update(|cx| view.read(cx).reader().read(cx).session().all_rows().len())
    };

    assert_eq!(rows(cx), 120, "120 段长文必须一次全部渲染");
    assert_eq!(offset(cx), px(0.), "打开文章从顶部开始");

    // 一路 PageDown：每次都继续向下，直到文末（再按不再移动）为止。
    let mut last = offset(cx);
    let mut steps = 0;
    loop {
        cx.simulate_keystrokes(window, "pagedown");
        let now = offset(cx);
        assert_eq!(page(cx), 0, "连续滚动不能切段落页");
        if now == last {
            break;
        }
        assert!(now < last, "PageDown 必须持续向下滚动");
        last = now;
        steps += 1;
        assert!(steps < 200, "长文应在有限次 PageDown 内读完，不能死循环");
    }
    assert!(
        steps > 3,
        "120 段正文应跨多屏，实际只滚了 {steps} 屏（可能只渲染了一页）"
    );
    assert!(last < px(-1000.), "必须真的滚到长文深处，实际 {last:?}");

    // 到文末后 PageUp 能往回滚，证明滚动范围覆盖全文而不是卡死。
    cx.simulate_keystrokes(window, "pageup");
    assert!(offset(cx) > last, "PageUp 应向上回滚");
}

/// 点击缓存按钮后：后台回调进度、完成后显示提示，且不会切换当前订阅。
struct FakeCache {
    calls: Arc<AtomicUsize>,
}

impl OfflineCacheService for FakeCache {
    fn cache_feed(
        &self,
        feed_id: i64,
        progress: &(dyn Fn(OfflineCacheProgress) + Send + Sync),
    ) -> Result<OfflineCacheReport, TelaError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        progress(OfflineCacheProgress {
            feed_id,
            total: 2,
            done: 1,
            originals: 1,
            ..OfflineCacheProgress::default()
        });
        progress(OfflineCacheProgress {
            feed_id,
            total: 2,
            done: 2,
            originals: 2,
            translations: 1,
            finished: false,
            ..OfflineCacheProgress::default()
        });
        Ok(OfflineCacheReport {
            total: 2,
            originals_cached: 2,
            translations_cached: 1,
            ..OfflineCacheReport::default()
        })
    }
}

#[gpui::test]
fn cache_button_runs_prefetch_and_reports_progress(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let calls = Arc::new(AtomicUsize::new(0));
    let data = Arc::new(FakeData {
        feeds: vec![feed(1, "Tela 示例博客", 2, 2), feed(2, "技术随笔", 1, 1)],
        articles: HashMap::from([(
            1,
            vec![article(42, 1, "Hello Tela"), article(43, 1, "长章节")],
        )]),
        content_key: Some(MOCK_CONTENT_KEY.to_string()),
        ..FakeData::default()
    });
    let data_for_view: Arc<dyn ShellData> = data;
    let cache: Arc<dyn OfflineCacheService> = Arc::new(FakeCache {
        calls: Arc::clone(&calls),
    });
    let (window, view) = open_window(cx, move |_, cx| {
        cx.new(|cx| ShellView::new_with_services(data_for_view, content(), None, Some(cache), cx))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();

    // 直接调用按钮动作（GPUI 的元素级点击在 headless 下不如方法调用稳定）。
    view.update(cx, |view, cx| view.cache_feed(1, cx));
    cx.run_until_parked();
    // 进度轮询任务每 200ms 收一次尾。
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let progress = view
        .read_with(cx, |view, _| view.cache_progress(1).cloned())
        .expect("应记录订阅 1 的缓存进度");
    assert!(progress.finished);
    assert_eq!(progress.originals, 2);
    assert_eq!(progress.translations, 1);
    let notice = view
        .read_with(cx, |view, _| view.cache_notice().map(str::to_string))
        .expect("完成后应有一句提示");
    assert!(notice.contains("Tela 示例博客"), "{notice}");
    assert!(notice.contains("新缓存原文 2 篇"), "{notice}");
    // 缓存按钮不应顺带切换订阅。
    assert_eq!(
        view.read_with(cx, |view, _| view.model().selected_feed),
        Some(1)
    );
}

/// “我的推荐”栏：从侧栏切入、选中推荐打开文章、取消推荐后列表与界面同步。
#[gpui::test]
fn recommendations_pane_lists_opens_and_cancels(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let items = Arc::new(Mutex::new(vec![
        recommendation(42, "Hello Tela"),
        recommendation(43, "长章节"),
    ]));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let data = Arc::new(FakeData {
        feeds: vec![feed(1, "Tela 示例博客", 2, 2)],
        articles: HashMap::from([(
            1,
            vec![article(42, 1, "Hello Tela"), article(43, 1, "长章节")],
        )]),
        content_key: Some(MOCK_CONTENT_KEY.to_string()),
        recommendations: Some(Arc::clone(&items)),
        bookmarks: None,
    });
    let data_for_view: Arc<dyn ShellData> = data;
    let recommender: Arc<dyn RecommendationService> = Arc::new(FakeRecommender {
        items: Arc::clone(&items),
        calls: Arc::clone(&calls),
    });
    let (window, view) = open_window(cx, move |_, cx| {
        cx.new(|cx| {
            ShellView::new_with_recommendations(
                data_for_view,
                content(),
                None,
                None,
                Some(recommender),
                cx,
            )
        })
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        assert_eq!(view.read(cx).model().recommendations.len(), 2);

        let shown = view.update(cx, |view, cx| {
            let changed = view.show_recommendations(cx);
            cx.notify();
            changed
        });
        assert!(shown);
        window.draw(cx).clear(cx);
        assert_eq!(view.read(cx).model().section, ShellSection::Recommendations);

        let selected = view.update(cx, |view, cx| {
            let changed = view.select_recommendation(42, cx);
            cx.notify();
            changed
        });
        assert!(selected);
        assert_eq!(view.read(cx).reader().read(cx).article_id(), Some(42));
        window.draw(cx).clear(cx);
    })
    .unwrap();

    view.update(cx, |view, cx| view.cancel_recommendation(42, cx));
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();

    assert_eq!(items.lock().unwrap().len(), 1);
    assert_eq!(
        cx.update(|cx| view.read(cx).model().recommendations.len()),
        1
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.recommend_notice().map(str::to_string))
            .as_deref(),
        Some("已取消推荐。")
    );
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[(42, None)],
        "取消推荐只应产生一次写调用"
    );
}

/// 阅读面板的推荐弹窗：空推荐语也能推荐；再次打开可取消推荐。
#[gpui::test]
fn reader_recommend_dialog_recommends_and_unrecommends(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let items = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let data = Arc::new(FakeData {
        feeds: vec![feed(1, "Tela 示例博客", 1, 1)],
        articles: HashMap::from([(1, vec![article(42, 1, "Hello Tela")])]),
        content_key: Some(MOCK_CONTENT_KEY.to_string()),
        recommendations: Some(Arc::clone(&items)),
        bookmarks: None,
    });
    let data_for_view: Arc<dyn ShellData> = data;
    let recommender: Arc<dyn RecommendationService> = Arc::new(FakeRecommender {
        items: Arc::clone(&items),
        calls: Arc::clone(&calls),
    });
    let (window, view) = open_window(cx, move |_, cx| {
        cx.new(|cx| {
            ShellView::new_with_recommendations(
                data_for_view,
                content(),
                None,
                None,
                Some(recommender),
                cx,
            )
        })
    });
    let reader = view.read_with(cx, |view, _| view.reader());

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        reader.update(cx, |reader, cx| {
            reader.open_recommend_dialog(window, cx);
            assert!(reader.recommend_dialog_open());
        });
        // 弹窗是模态的：t 不切视图、j 不换文章。
        assert!(!reader.update(cx, |reader, cx| reader.handle_key("t", cx)));
        assert!(!view.update(cx, |view, cx| view.handle_shortcut("j", false, window, cx)));
        window.draw(cx).clear(cx);
    })
    .unwrap();

    reader.update(cx, |reader, cx| reader.submit_recommendation(cx));
    assert_eq!(calls.lock().unwrap().as_slice(), &[(42, None)]);
    assert!(cx.update(|cx| !reader.read(cx).recommend_dialog_open()));
    assert!(cx.update(|cx| reader.read(cx).recommendation().is_some()));
    assert_eq!(
        cx.update(|cx| reader.read(cx).recommend_notice().map(str::to_string))
            .as_deref(),
        Some("已推荐。")
    );

    cx.update_window(window, |_, window, cx| {
        reader.update(cx, |reader, cx| {
            reader.open_recommend_dialog(window, cx);
            assert!(reader.recommend_dialog_open(), "已推荐时应能编辑");
        });
    })
    .unwrap();
    reader.update(cx, |reader, cx| reader.confirm_unrecommend(cx));
    assert!(cx.update(|cx| reader.read(cx).recommendation().is_none()));
    assert!(items.lock().unwrap().is_empty());
    assert_eq!(calls.lock().unwrap().as_slice(), &[(42, None), (42, None)]);

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
}

// ---------------------------------------------------------------------------
// 我的收藏（Shift+B）
// ---------------------------------------------------------------------------

/// 收藏服务的测试替身：记录调用并同步修改共享的收藏列表。
struct FakeBookmarks {
    items: Arc<Mutex<Vec<BookmarkItem>>>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl BookmarkService for FakeBookmarks {
    fn add_bookmark(&self, url: &str) -> Result<BookmarkItem, TelaError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(url.to_string());
        }
        let mut items = self.items.lock().unwrap();
        let item = BookmarkItem {
            id: 100 + items.len() as i64,
            url: url.to_string(),
            title: "收藏的网页".to_string(),
            site: "example.com".to_string(),
            image_count: 3,
            created_at: 1_761_350_400_000,
        };
        items.push(item.clone());
        Ok(item)
    }

    fn remove_bookmark(&self, bookmark_id: i64) -> Result<(), TelaError> {
        if let Ok(mut items) = self.items.lock() {
            items.retain(|item| item.id != bookmark_id);
        }
        Ok(())
    }
}

fn bookmark(id: i64, title: &str, created_at: i64) -> BookmarkItem {
    BookmarkItem {
        id,
        url: format!("https://example.com/bookmarks/{id}"),
        title: title.to_string(),
        site: "example.com".to_string(),
        image_count: 2,
        created_at,
    }
}

/// Shift+B 打开收藏弹窗 → 输入网址 → 提交 → 列表刷新、阅读面板打开收藏正文。
#[gpui::test]
fn shift_b_opens_dialog_and_saves_a_bookmark(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let items = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let data = Arc::new(FakeData {
        feeds: vec![feed(1, "Tela 示例博客", 1, 1)],
        articles: HashMap::from([(1, vec![article(42, 1, "Hello Tela")])]),
        content_key: Some(MOCK_CONTENT_KEY.to_string()),
        bookmarks: Some(Arc::clone(&items)),
        ..FakeData::default()
    });
    let data_for_view: Arc<dyn ShellData> = data;
    let service: Arc<dyn BookmarkService> = Arc::new(FakeBookmarks {
        items: Arc::clone(&items),
        calls: Arc::clone(&calls),
    });
    let (window, view) = open_window(cx, move |_, cx| {
        cx.new(|cx| {
            ShellView::new_with_bookmarks(
                data_for_view,
                content(),
                None,
                None,
                None,
                Some(service),
                cx,
            )
        })
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        assert!(!view.read(cx).bookmark_dialog_open());
    })
    .unwrap();

    // Shift+B：弹窗出现，输入框拿到焦点。
    cx.simulate_keystrokes(window, "shift-b");
    assert!(
        cx.update(|cx| view.read(cx).bookmark_dialog_open()),
        "Shift+B 应打开收藏弹窗"
    );

    cx.simulate_input(window, "https://example.com/saved");
    let dialog = cx
        .update(|cx| view.read(cx).bookmark_dialog())
        .expect("弹窗实体存在");
    assert_eq!(
        cx.update(|cx| dialog.read(cx).url_text(cx)),
        "https://example.com/saved",
        "输入的网址应进入输入框"
    );

    dialog.update(cx, |dialog, cx| dialog.submit(cx));
    // 后台下载任务 + 前台回报：轮询到弹窗关闭为止（替身是即时的）。
    let mut saved = false;
    for _ in 0..100 {
        cx.run_until_parked();
        if !cx.update(|cx| view.read(cx).bookmark_dialog_open()) {
            saved = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(saved, "收藏成功后弹窗应关闭");
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &["https://example.com/saved"]
    );
    assert_eq!(items.lock().unwrap().len(), 1);

    let model = cx.update(|cx| view.read(cx).model().clone());
    assert_eq!(model.bookmarks.len(), 1, "收藏列表应立刻刷新");
    assert!(
        view.read_with(cx, |view, _| view.bookmark_notice().map(str::to_string))
            .as_deref()
            .is_some_and(|notice| notice.contains("收藏的网页")),
        "应给出收藏成功的提示"
    );

    // 切到「我的收藏」并选中：阅读面板读的是收藏正文（bookmark_id 非空）。
    cx.update_window(window, |_, window, cx| {
        view.update(cx, |view, cx| {
            assert!(view.show_bookmarks(cx));
            assert!(view.select_bookmark(100, cx));
            cx.notify();
        });
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.run_until_parked();

    let reader = cx.update(|cx| view.read(cx).reader());
    assert_eq!(
        cx.update(|cx| reader.read(cx).bookmark_id()),
        Some(100),
        "阅读面板应切到收藏正文"
    );
    // 收藏正文必须真的进入阅读态（不能停在“选择一篇文章开始阅读”的空态）。
    let mut loaded = 0;
    for _ in 0..50 {
        cx.run_until_parked();
        loaded = cx.update(|cx| reader.read(cx).session().total_paragraphs());
        if loaded > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(loaded > 0, "收藏的正文应被阅读面板加载出来");
    assert_eq!(
        cx.update(|cx| view.read(cx).model().section),
        ShellSection::Bookmarks
    );

    // 弹窗关闭后 j/k 恢复工作。
    cx.simulate_keystrokes(window, "j");
    assert_eq!(
        cx.update(|cx| view.read(cx).model().selected_bookmark),
        Some(100),
        "收藏栏里 j 只在收藏项之间移动"
    );
}

/// 「我的收藏」列表：侧栏入口、删除按钮（直接调用动作）与阅读面板回退。
#[gpui::test]
fn bookmarks_pane_lists_and_removes(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let items = Arc::new(Mutex::new(vec![
        bookmark(7, "收藏七", 2_000),
        bookmark(8, "收藏八", 1_000),
    ]));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let data = Arc::new(FakeData {
        feeds: vec![feed(1, "A", 0, 0)],
        articles: HashMap::from([(1, Vec::new())]),
        content_key: Some(MOCK_CONTENT_KEY.to_string()),
        bookmarks: Some(Arc::clone(&items)),
        ..FakeData::default()
    });
    let data_for_view: Arc<dyn ShellData> = data;
    let service: Arc<dyn BookmarkService> = Arc::new(FakeBookmarks {
        items: Arc::clone(&items),
        calls: Arc::clone(&calls),
    });
    let (window, view) = open_window(cx, move |_, cx| {
        cx.new(|cx| {
            ShellView::new_with_bookmarks(
                data_for_view,
                content(),
                None,
                None,
                None,
                Some(service),
                cx,
            )
        })
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        assert_eq!(view.read(cx).model().bookmarks.len(), 2);
        assert_eq!(view.read(cx).model().status_line(), "A · 0 篇 · 0 未读");

        view.update(cx, |view, cx| {
            assert!(view.show_bookmarks(cx));
            assert!(view.select_bookmark(7, cx));
            cx.notify();
        });
        window.draw(cx).clear(cx);
        assert_eq!(view.read(cx).model().section, ShellSection::Bookmarks);
        assert_eq!(view.read(cx).reader().read(cx).bookmark_id(), Some(7));

        // 删除正在读的收藏：列表少一条，阅读面板回到空态。
        view.update(cx, |view, cx| view.remove_bookmark(7, cx));
        window.draw(cx).clear(cx);
        assert_eq!(view.read(cx).model().bookmarks.len(), 1);
        assert_eq!(view.read(cx).reader().read(cx).bookmark_id(), None);
        assert_eq!(
            view.read(cx)
                .bookmark_notice()
                .map(str::to_string)
                .as_deref(),
            Some("已从收藏中移除。")
        );
    })
    .unwrap();
    assert!(items.lock().unwrap().iter().all(|item| item.id != 7));
}
