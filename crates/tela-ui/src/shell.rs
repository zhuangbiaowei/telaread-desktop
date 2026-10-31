//! 三栏外壳的纯视图模型（S2，切片 C11 的前半）。
//!
//! 这一层**不依赖 GPUI**：订阅列表、文章列表、选中态、空态与错误态都是普通数据，
//! 可以在无显示器环境（含 CI）完整测试。GUI 视图只负责把这些数据画成元素，
//! 因此“选中逻辑错了”和“布局画错了”能分开定位。
//!
//! 数据来源由 [`ShellData`] 抽象；`tela-app` 用本地 `Store` 实现它，
//! 测试用内存 fake 实现它。这样 UI 层不直接写 SQL，也不直接发 HTTP。

use tela_content::api::ChapterRequest;
use tela_proto::TelaError;

/// 左侧订阅列表的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedItem {
    /// feed id（本地主键）。
    pub id: i64,
    /// 展示标题：feed 标题优先，缺失时回退到 feed URL。
    pub title: String,
    /// feed 抓取地址（标题回退与诊断用）。
    pub feed_url: String,
    /// 未读文章数。
    pub unread: usize,
    /// 文章总数。
    pub article_count: usize,
    /// 最近同步状态（`active` / `error` / `pending`）。
    pub status: String,
}

/// 中间文章列表的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleItem {
    /// 文章 id。
    pub id: i64,
    /// 所属 feed id。
    pub feed_id: i64,
    /// 原文地址；缺失时阅读面板不显示“打开原文”。
    pub url: Option<String>,
    /// 标题（服务端保证非空）。
    pub title: String,
    /// 是否未读。
    pub unread: bool,
    /// 发布时间；缺失时界面不显示时间。
    pub published_at: Option<i64>,
    /// 摘要；可空。
    pub excerpt: Option<String>,
}

impl ArticleItem {
    /// 列表里展示的发表时间（UTC 日期）；没有发布时间时为 `None`。
    pub fn published_label(&self) -> Option<String> {
        self.published_at.map(format_published_at)
    }
}

/// epoch 时间 → `YYYY-MM-DD`。
///
/// v1 wire 的 `publishedAt` 是毫秒，但历史 fixture 和演示数据里也出现过秒；
/// 小于 1e11 的值按秒处理。只显示 UTC 日期，避免引入时区依赖——列表排序
/// 仍由服务端的 `sortAt` 决定，日期只用于让人看出新旧。
pub fn format_published_at(timestamp: i64) -> String {
    let ms = if timestamp.unsigned_abs() < 100_000_000_000 {
        timestamp.saturating_mul(1000)
    } else {
        timestamp
    };
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

/// 外壳的数据来源：只读，不发请求、不写库。
///
/// 实现方负责把本地库的行解成 [`FeedItem`] / [`ArticleItem`]；
/// UI 层不关心 SQLite 或 wire JSON。
pub trait ShellData: Send + Sync {
    /// 全部在用订阅，按 feed id 升序。
    fn feeds(&self) -> Result<Vec<FeedItem>, TelaError>;

    /// 某个订阅下的文章，按列表排序时间倒序。
    fn articles(&self, feed_id: i64) -> Result<Vec<ArticleItem>, TelaError>;

    /// 某篇文章对应的正文请求（正文 key + 目标语言）。
    fn chapter_request(&self, article_id: i64) -> Result<ChapterRequest, TelaError>;
}

/// 三栏外壳的当前状态。
///
/// 约定：
/// - `selected_feed` 永远指向 `feeds` 里存在的 id（除非列表为空）；
/// - 切换 feed 会重新加载文章，并把 `selected_article` 重置为该 feed 的第一篇；
/// - 任何错误都保留在 `feeds_error` / `articles_error` 里，不清空另一侧已加载的数据
///   （订阅列表失败不该把已经读到的文章也抹掉）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ShellViewModel {
    /// 订阅列表。
    pub feeds: Vec<FeedItem>,
    /// 当前 feed 的文章列表。
    pub articles: Vec<ArticleItem>,
    /// 当前选中的 feed。
    pub selected_feed: Option<i64>,
    /// 当前选中的文章。
    pub selected_article: Option<i64>,
    /// 订阅列表加载错误（展示在左侧空态/错误区）。
    pub feeds_error: Option<String>,
    /// 文章列表加载错误（展示在中间空态/错误区）。
    pub articles_error: Option<String>,
}

impl ShellViewModel {
    /// 空状态（首次加载前）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从数据源刷新订阅与文章；返回是否发生变化（供 GUI 决定是否重绘）。
    ///
    /// 选中项按 id 保留：刷新后原来选中的 feed/article 还在，就不跳走；
    /// 不在了才回退到第一项。这样后台同步完成后用户不会被强行拉回顶部。
    pub fn reload(&mut self, data: &dyn ShellData) -> bool {
        let before = self.clone();
        match data.feeds() {
            Ok(feeds) => {
                self.feeds = feeds;
                self.feeds_error = None;
                let selected = self
                    .selected_feed
                    .filter(|id| self.feeds.iter().any(|feed| feed.id == *id))
                    .or_else(|| self.feeds.first().map(|feed| feed.id));
                self.selected_feed = selected;
                self.reload_articles(data);
            }
            Err(error) => {
                self.feeds.clear();
                self.feeds_error = Some(error.to_string());
                self.selected_feed = None;
                self.articles.clear();
                self.selected_article = None;
                self.articles_error = None;
            }
        }
        *self != before
    }

    /// 选中一个订阅；id 不在列表里时忽略并返回 `false`。
    pub fn select_feed(&mut self, feed_id: i64, data: &dyn ShellData) -> bool {
        if self.selected_feed == Some(feed_id) || !self.feeds.iter().any(|feed| feed.id == feed_id)
        {
            return false;
        }
        self.selected_feed = Some(feed_id);
        self.selected_article = None;
        self.reload_articles(data);
        true
    }

    /// 选中一篇文章；id 不在当前列表里时忽略并返回 `false`。
    pub fn select_article(&mut self, article_id: i64) -> bool {
        if self.selected_article == Some(article_id)
            || !self.articles.iter().any(|article| article.id == article_id)
        {
            return false;
        }
        self.selected_article = Some(article_id);
        true
    }

    /// 下一篇（`j`）：到文章列表末尾停住，不跨订阅。
    pub fn next_article(&mut self) -> bool {
        self.step_article(1)
    }

    /// 上一篇（`k`）：到文章列表开头停住，不跨订阅。
    pub fn prev_article(&mut self) -> bool {
        self.step_article(-1)
    }

    /// 下一个博客（`Shift+J`）：切换订阅并加载它的第一篇文章。
    pub fn next_feed(&mut self, data: &dyn ShellData) -> bool {
        self.step_feed(1, data)
    }

    /// 上一个博客（`Shift+K`）：切换订阅并加载它的第一篇文章。
    pub fn prev_feed(&mut self, data: &dyn ShellData) -> bool {
        self.step_feed(-1, data)
    }

    fn step_article(&mut self, delta: isize) -> bool {
        if self.articles.is_empty() {
            return false;
        }
        let current = self
            .selected_article
            .and_then(|id| self.articles.iter().position(|article| article.id == id));
        let target = match current {
            Some(index) => index.saturating_add_signed(delta),
            None if delta > 0 => 0,
            None => self.articles.len() - 1,
        }
        .min(self.articles.len() - 1);
        if Some(target) == current {
            return false;
        }
        self.selected_article = Some(self.articles[target].id);
        true
    }

    fn step_feed(&mut self, delta: isize, data: &dyn ShellData) -> bool {
        if self.feeds.is_empty() {
            return false;
        }
        let current = self
            .selected_feed
            .and_then(|id| self.feeds.iter().position(|feed| feed.id == id));
        let target = match current {
            Some(index) => index.saturating_add_signed(delta),
            None if delta > 0 => 0,
            None => self.feeds.len() - 1,
        }
        .min(self.feeds.len() - 1);
        if Some(target) == current {
            return false;
        }
        let feed_id = self.feeds[target].id;
        self.select_feed(feed_id, data)
    }

    /// 当前选中的 feed。
    pub fn selected_feed_item(&self) -> Option<&FeedItem> {
        let id = self.selected_feed?;
        self.feeds.iter().find(|feed| feed.id == id)
    }

    /// 当前选中的文章。
    pub fn selected_article_item(&self) -> Option<&ArticleItem> {
        let id = self.selected_article?;
        self.articles.iter().find(|article| article.id == id)
    }

    /// 全部订阅的未读总数。
    pub fn unread_total(&self) -> usize {
        self.feeds.iter().map(|feed| feed.unread).sum()
    }

    /// 是否处于“没有任何订阅”的空态（错误态不算空态）。
    pub fn is_empty(&self) -> bool {
        self.feeds.is_empty() && self.feeds_error.is_none()
    }

    /// 顶部状态栏文案。
    pub fn status_line(&self) -> String {
        if let Some(error) = &self.feeds_error {
            return format!("订阅列表加载失败：{error}");
        }
        match self.selected_feed_item() {
            Some(feed) => format!(
                "{} · {} 篇 · {} 未读",
                feed.title, feed.article_count, feed.unread
            ),
            None => "还没有订阅".to_string(),
        }
    }

    /// 按 `selected_feed` 重新加载文章，并在必要时回退到第一篇。
    fn reload_articles(&mut self, data: &dyn ShellData) {
        let Some(feed_id) = self.selected_feed else {
            self.articles.clear();
            self.selected_article = None;
            self.articles_error = None;
            return;
        };

        match data.articles(feed_id) {
            Ok(articles) => {
                self.articles = articles;
                self.articles_error = None;
                let selected = self
                    .selected_article
                    .filter(|id| self.articles.iter().any(|article| article.id == *id))
                    .or_else(|| self.articles.first().map(|article| article.id));
                self.selected_article = selected;
            }
            Err(error) => {
                self.articles.clear();
                self.selected_article = None;
                self.articles_error = Some(error.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn article(published_at: Option<i64>) -> ArticleItem {
        ArticleItem {
            id: 1,
            feed_id: 1,
            url: None,
            title: "标题".to_string(),
            unread: true,
            published_at,
            excerpt: None,
        }
    }

    #[test]
    fn published_label_uses_utc_dates_for_seconds_and_milliseconds() {
        assert_eq!(format_published_at(1_761_350_400), "2025-10-25");
        assert_eq!(format_published_at(1_761_350_400_000), "2025-10-25");
        assert_eq!(format_published_at(0), "1970-01-01");
        assert_eq!(
            article(Some(1_761_350_700)).published_label().as_deref(),
            Some("2025-10-25")
        );
    }

    #[test]
    fn missing_published_at_has_no_label() {
        assert_eq!(article(None).published_label(), None);
    }
}
