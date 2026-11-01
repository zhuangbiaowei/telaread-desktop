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
    ArticleItem, FeedItem, OfflineCacheProgress, OfflineCacheReport, OfflineCacheService,
    RecommendationItem, RecommendationService, ShellData, ShellSection,
};

#[derive(Default)]
struct FakeData {
    feeds: Vec<FeedItem>,
    articles: HashMap<i64, Vec<ArticleItem>>,
    /// 指定所有文章共用的正文 key；默认用 article id（多数测试只验证列表）。
    content_key: Option<String>,
    /// 与 `FakeRecommender` 共享的推荐列表，模拟本地库的实时变化。
    recommendations: Option<Arc<Mutex<Vec<RecommendationItem>>>>,
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
        assert!(!view.update(cx, |view, cx| view.handle_shortcut("j", false, cx)));
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
