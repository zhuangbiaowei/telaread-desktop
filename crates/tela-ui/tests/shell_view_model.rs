//! S2 三栏外壳纯视图模型测试（不依赖 GPUI / 显示器）。
//!
//! 覆盖：空态、首次选中、切换 feed 重载文章、刷新保留选中、错误态隔离。
//! 这些是 GUI 行为测试的基础；GUI 层只验证“数据 → 元素”的映射。

use std::collections::HashMap;

use tela_content::api::ChapterRequest;
use tela_proto::TelaError;
use tela_ui::shell::{ArticleItem, FeedItem, ShellData, ShellViewModel};

#[derive(Default)]
struct FakeData {
    feeds: Option<Result<Vec<FeedItem>, String>>,
    articles: HashMap<i64, Result<Vec<ArticleItem>, String>>,
}

impl FakeData {
    fn with(feeds: Vec<FeedItem>, articles: Vec<(i64, Vec<ArticleItem>)>) -> Self {
        Self {
            feeds: Some(Ok(feeds)),
            articles: articles
                .into_iter()
                .map(|(id, rows)| (id, Ok(rows)))
                .collect(),
        }
    }
}

fn err(message: &str) -> TelaError {
    TelaError::Content {
        article: 0,
        content_key: String::new(),
        detail: message.to_string(),
    }
}

impl ShellData for FakeData {
    fn feeds(&self) -> Result<Vec<FeedItem>, TelaError> {
        match &self.feeds {
            Some(Ok(feeds)) => Ok(feeds.clone()),
            Some(Err(error)) => Err(err(error)),
            None => Ok(Vec::new()),
        }
    }

    fn articles(&self, feed_id: i64) -> Result<Vec<ArticleItem>, TelaError> {
        match self.articles.get(&feed_id) {
            Some(Ok(articles)) => Ok(articles.clone()),
            Some(Err(error)) => Err(err(error)),
            None => Ok(Vec::new()),
        }
    }

    fn chapter_request(&self, article_id: i64) -> Result<ChapterRequest, TelaError> {
        Ok(ChapterRequest::new(article_id.to_string(), "zh-Hans"))
    }
}

fn feed(id: i64, title: &str, unread: usize, article_count: usize) -> FeedItem {
    FeedItem {
        id,
        title: title.to_string(),
        feed_url: format!("https://example.com/{id}.xml"),
        unread,
        article_count,
        status: "active".to_string(),
    }
}

fn article(id: i64, feed_id: i64, title: &str, unread: bool) -> ArticleItem {
    ArticleItem {
        id,
        feed_id,
        title: title.to_string(),
        unread,
        published_at: Some(1_761_350_400 + id),
        excerpt: None,
    }
}

#[test]
fn empty_store_is_an_explicit_empty_state() {
    let data = FakeData::default();
    let mut model = ShellViewModel::new();

    assert!(model.is_empty());
    // 空库刷新后状态仍是“空”，因此不报告变化；GUI 首帧照样会画空态。
    assert!(!model.reload(&data));
    assert!(model.is_empty());
    assert_eq!(model.status_line(), "还没有订阅");
    assert!(model.feeds_error.is_none());
    assert!(model.selected_feed.is_none());
}

#[test]
fn first_feed_and_first_article_are_selected_on_reload() {
    let data = FakeData::with(
        vec![feed(1, "Tela 示例博客", 2, 3), feed(2, "技术随笔", 0, 1)],
        vec![
            (
                1,
                vec![
                    article(42, 1, "Hello Tela", true),
                    article(43, 1, "长章节", true),
                ],
            ),
            (2, vec![article(45, 2, "Rust 周报", false)]),
        ],
    );
    let mut model = ShellViewModel::new();

    assert!(model.reload(&data));
    assert_eq!(model.selected_feed, Some(1));
    assert_eq!(model.selected_article, Some(42));
    assert_eq!(model.unread_total(), 2);
    assert_eq!(model.status_line(), "Tela 示例博客 · 3 篇 · 2 未读");
    assert_eq!(
        model
            .selected_article_item()
            .map(|article| article.title.as_str()),
        Some("Hello Tela")
    );
}

#[test]
fn selecting_another_feed_reloads_articles_and_resets_article_selection() {
    let data = FakeData::with(
        vec![feed(1, "A", 1, 1), feed(2, "B", 1, 1)],
        vec![
            (1, vec![article(10, 1, "A1", true)]),
            (2, vec![article(20, 2, "B1", true)]),
        ],
    );
    let mut model = ShellViewModel::new();
    model.reload(&data);

    assert!(model.select_feed(2, &data));
    assert_eq!(model.selected_feed, Some(2));
    assert_eq!(model.selected_article, Some(20));
    assert_eq!(model.articles.len(), 1);

    // 重复选中同一个 feed 不重载、不报告变化。
    assert!(!model.select_feed(2, &data));
    // 不存在的 feed 被忽略。
    assert!(!model.select_feed(999, &data));
}

#[test]
fn reload_keeps_the_user_selection_when_it_still_exists() {
    let mut model = ShellViewModel::new();
    let first = FakeData::with(
        vec![feed(1, "A", 1, 2), feed(2, "B", 1, 1)],
        vec![
            (
                1,
                vec![article(10, 1, "A1", true), article(11, 1, "A2", true)],
            ),
            (2, vec![article(20, 2, "B1", true)]),
        ],
    );
    model.reload(&first);
    model.select_feed(2, &first);
    model.select_article(20);

    // 后台同步完成后标题/未读数变化，但 id 不变：选中项必须保留。
    let refreshed = FakeData::with(
        vec![feed(1, "A", 0, 2), feed(2, "B", 0, 1)],
        vec![
            (
                1,
                vec![article(10, 1, "A1", false), article(11, 1, "A2", false)],
            ),
            (2, vec![article(20, 2, "B1", false)]),
        ],
    );
    assert!(model.reload(&refreshed));
    assert_eq!(model.selected_feed, Some(2));
    assert_eq!(model.selected_article, Some(20));
}

#[test]
fn feed_error_clears_selection_but_article_error_keeps_feeds() {
    let mut data = FakeData::with(
        vec![feed(1, "A", 1, 1)],
        vec![(1, vec![article(10, 1, "A1", true)])],
    );
    let mut model = ShellViewModel::new();
    model.reload(&data);

    data.articles.insert(1, Err("正文索引损坏".to_string()));
    model.reload(&data);
    assert_eq!(model.feeds.len(), 1);
    assert!(model.articles.is_empty());
    assert_eq!(model.selected_article, None);
    assert!(model.articles_error.is_some());
    assert!(model.feeds_error.is_none());

    data.feeds = Some(Err("订阅表损坏".to_string()));
    model.reload(&data);
    assert!(model.feeds.is_empty());
    assert_eq!(model.selected_feed, None);
    assert!(model.feeds_error.is_some());
    assert!(model.status_line().contains("订阅列表加载失败"));
    assert!(!model.is_empty(), "错误态不能被当成空态");
}

/// j/k 在文章列表内移动并夹在两端；Shift+j/k 切换订阅并回到该订阅第一篇。
#[test]
fn article_and_feed_navigation_step_and_clamp() {
    let data = FakeData::with(
        vec![feed(1, "A", 3, 3), feed(2, "B", 2, 2)],
        vec![
            (
                1,
                vec![
                    article(10, 1, "A1", true),
                    article(11, 1, "A2", true),
                    article(12, 1, "A3", true),
                ],
            ),
            (
                2,
                vec![article(20, 2, "B1", true), article(21, 2, "B2", true)],
            ),
        ],
    );
    let mut model = ShellViewModel::new();
    model.reload(&data);

    assert_eq!(model.selected_article, Some(10));
    assert!(model.next_article());
    assert_eq!(model.selected_article, Some(11));
    assert!(model.next_article());
    assert_eq!(model.selected_article, Some(12));
    assert!(!model.next_article(), "末篇不应越界或跨订阅");
    assert!(model.prev_article());
    assert_eq!(model.selected_article, Some(11));

    assert!(model.next_feed(&data));
    assert_eq!(model.selected_feed, Some(2));
    assert_eq!(model.selected_article, Some(20), "换博客后打开第一篇");
    assert!(!model.next_feed(&data), "最后一个博客不应越界");
    assert!(model.prev_feed(&data));
    assert_eq!(model.selected_feed, Some(1));
    assert_eq!(model.selected_article, Some(10));
}
