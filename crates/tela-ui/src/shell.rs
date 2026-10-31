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

/// “我的推荐”里的一条：成员公开推荐的一篇文章。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecommendationItem {
    /// 文章 id。
    pub article_id: i64,
    /// 文章标题（优先当前语言译文）。
    pub title: String,
    /// 原文地址；缺失时阅读面板不显示“打开原文”。
    pub url: Option<String>,
    /// 所属 feed id。
    pub feed_id: i64,
    /// 博客展示名（feed 标题优先，缺失时回退到站点/地址）。
    pub feed_title: String,
    /// 推荐语；可空。
    pub note: Option<String>,
    /// 推荐时间（epoch 毫秒）。
    pub created_at: i64,
}

impl RecommendationItem {
    /// 列表里展示的推荐日期（UTC）。
    pub fn created_label(&self) -> String {
        format_published_at(self.created_at)
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

    /// 成员当前有效的公开推荐，按推荐时间倒序。
    ///
    /// 默认实现返回空列表：只读测试/嵌入方不必知道推荐功能；
    /// 真实装配由 `tela-app` 从本地库的 `recommendations` 行构造。
    fn recommendations(&self) -> Result<Vec<RecommendationItem>, TelaError> {
        Ok(Vec::new())
    }

    /// 某一篇文章是否已被成员推荐；未推荐时返回 `None`。
    fn recommendation(&self, article_id: i64) -> Result<Option<RecommendationItem>, TelaError> {
        Ok(self
            .recommendations()?
            .into_iter()
            .find(|item| item.article_id == article_id))
    }
}

/// 推荐写能力：供阅读面板的“推荐”按钮与推荐列表的“取消推荐”调用。
///
/// 实现方先做本地乐观更新（写本地库并入离线队列），再由外壳触发同步；
/// 网络失败不影响界面，队列会在下一次同步时原样重发。
pub trait RecommendationService: Send + Sync {
    /// 推荐一篇文章；`note` 为空表示不带推荐语。已推荐时表示更新推荐语。
    fn recommend(&self, article_id: i64, note: Option<String>) -> Result<(), TelaError>;

    /// 撤回一篇文章的推荐（未推荐时也视为成功）。
    fn unrecommend(&self, article_id: i64) -> Result<(), TelaError>;
}

/// 订阅级离线缓存进度（左侧博客名旁的按钮显示它）。
///
/// `done/total` 是“已处理”的文章数，不等同于“已完整缓存”：译文仍在生成的
/// 文章会计入 `pending`，下次点击按钮会继续补齐。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OfflineCacheProgress {
    /// 订阅 id。
    pub feed_id: i64,
    /// 该订阅的文章总数。
    pub total: usize,
    /// 已处理文章数（0..=total）。
    pub done: usize,
    /// 本次新缓存的原文数。
    pub originals: usize,
    /// 本次新缓存的译文数。
    pub translations: usize,
    /// 本次新缓存的图片数。
    pub images: usize,
    /// 图片下载失败数（网络/404）。
    pub images_failed: usize,
    /// 原文与译文都已缓存、本次跳过的篇数。
    pub skipped: usize,
    /// 失败篇数（网络/本地库错误）。
    pub failed: usize,
    /// 仍在生成、需要下次继续的译文数。
    pub pending: usize,
    /// 后台任务是否已结束。
    pub finished: bool,
}

impl OfflineCacheProgress {
    /// 缓存中（任务未结束）。
    pub fn is_running(&self) -> bool {
        !self.finished
    }

    /// 左侧按钮上的一行短文案。
    pub fn button_label(&self) -> String {
        if !self.finished {
            if self.total == 0 {
                return "缓存中…".to_string();
            }
            return format!("{}/{}", self.done, self.total);
        }
        if self.failed > 0 || self.images_failed > 0 {
            return "重试".to_string();
        }
        if self.pending > 0 {
            return format!("待续 {}", self.pending);
        }
        "已缓存".to_string()
    }
}

/// 一次订阅离线缓存的最终结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OfflineCacheReport {
    /// 该订阅的文章总数（不含没有正文 key 的文章）。
    pub total: usize,
    /// 新缓存的原文数。
    pub originals_cached: usize,
    /// 新缓存的译文数。
    pub translations_cached: usize,
    /// 新缓存的图片数。
    pub images_cached: usize,
    /// 图片下载失败数。
    pub images_failed: usize,
    /// 原文与译文都已缓存、本次跳过的篇数。
    pub skipped: usize,
    /// 失败篇数。
    pub failed: usize,
    /// 译文仍在生成、需要下次点击继续的篇数。
    pub pending: usize,
    /// 失败/额度提示（最多保留若干条）。
    pub errors: Vec<String>,
}

impl OfflineCacheReport {
    /// 状态栏/提示里的一句话总结。
    pub fn summary(&self) -> String {
        let mut summary = format!(
            "共 {} 篇：新缓存原文 {} 篇、译文 {} 篇",
            self.total, self.originals_cached, self.translations_cached
        );
        if self.images_cached > 0 {
            summary.push_str(&format!("、图片 {} 张", self.images_cached));
        }
        if self.skipped > 0 {
            summary.push_str(&format!("，已缓存 {} 篇", self.skipped));
        }
        if self.pending > 0 {
            summary.push_str(&format!("，仍待翻译 {} 篇", self.pending));
        }
        if self.failed > 0 {
            summary.push_str(&format!("，失败 {} 篇", self.failed));
        }
        if self.images_failed > 0 {
            summary.push_str(&format!("，图片失败 {} 张", self.images_failed));
        }
        summary
    }
}

/// 离线缓存能力：供左侧博客列表的“缓存”按钮调用。
///
/// 实现方在后台线程里把该订阅下所有文章的原文与译文写入本地库；
/// 回调会被多次调用，UI 据此显示进度。调用是阻塞的。
pub trait OfflineCacheService: Send + Sync {
    /// 缓存该订阅下所有未缓存的原文与译文。
    fn cache_feed(
        &self,
        feed_id: i64,
        progress: &(dyn Fn(OfflineCacheProgress) + Send + Sync),
    ) -> Result<OfflineCacheReport, TelaError>;
}

/// 中间栏当前显示哪个列表。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ShellSection {
    /// 当前订阅的文章列表（默认）。
    #[default]
    Feed,
    /// 成员自己的推荐列表。
    Recommendations,
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
    /// 当前 feed 的选中项。
    pub selected_feed: Option<i64>,
    /// 当前 feed 文章列表的选中项。
    pub selected_article: Option<i64>,
    /// 订阅列表加载错误（展示在左侧空态/错误区）。
    pub feeds_error: Option<String>,
    /// 文章列表加载错误（展示在中间空态/错误区）。
    pub articles_error: Option<String>,
    /// 中间栏当前是订阅文章还是“我的推荐”。
    pub section: ShellSection,
    /// 我的推荐（只含未撤回的项，按推荐时间倒序）。
    pub recommendations: Vec<RecommendationItem>,
    /// 推荐列表的选中项。
    pub selected_recommendation: Option<i64>,
    /// 推荐列表加载错误。
    pub recommendations_error: Option<String>,
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
        self.reload_recommendations(data);
        *self != before
    }

    /// 选中一个订阅并回到文章列表；id 不在列表里时忽略并返回 `false`。
    pub fn select_feed(&mut self, feed_id: i64, data: &dyn ShellData) -> bool {
        if !self.feeds.iter().any(|feed| feed.id == feed_id) {
            return false;
        }
        let mut changed = self.section != ShellSection::Feed;
        self.section = ShellSection::Feed;
        if self.selected_feed != Some(feed_id) {
            self.selected_feed = Some(feed_id);
            self.selected_article = None;
            self.reload_articles(data);
            changed = true;
        }
        changed
    }

    /// 打开“我的推荐”列表；返回视图是否需要重绘。
    pub fn show_recommendations(&mut self, data: &dyn ShellData) -> bool {
        let mut changed = self.section != ShellSection::Recommendations;
        self.section = ShellSection::Recommendations;
        changed |= self.reload_recommendations(data);
        changed
    }

    /// 选中一条推荐；文章不在推荐列表里时忽略。
    pub fn select_recommendation(&mut self, article_id: i64) -> bool {
        if self.selected_recommendation == Some(article_id)
            || !self
                .recommendations
                .iter()
                .any(|item| item.article_id == article_id)
        {
            return false;
        }
        self.section = ShellSection::Recommendations;
        self.selected_recommendation = Some(article_id);
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

    /// 下一篇（`j`）：到当前列表末尾停住，不跨订阅/不跳回订阅列表。
    pub fn next_article(&mut self) -> bool {
        match self.section {
            ShellSection::Feed => self.step_article(1),
            ShellSection::Recommendations => self.step_recommendation(1),
        }
    }

    /// 上一篇（`k`）：到当前列表开头停住。
    pub fn prev_article(&mut self) -> bool {
        match self.section {
            ShellSection::Feed => self.step_article(-1),
            ShellSection::Recommendations => self.step_recommendation(-1),
        }
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

    fn step_recommendation(&mut self, delta: isize) -> bool {
        if self.recommendations.is_empty() {
            return false;
        }
        let current = self.selected_recommendation.and_then(|id| {
            self.recommendations
                .iter()
                .position(|item| item.article_id == id)
        });
        let target = match current {
            Some(index) => index.saturating_add_signed(delta),
            None if delta > 0 => 0,
            None => self.recommendations.len() - 1,
        }
        .min(self.recommendations.len() - 1);
        if Some(target) == current {
            return false;
        }
        self.selected_recommendation = Some(self.recommendations[target].article_id);
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

    /// 当前选中的推荐项。
    pub fn selected_recommendation_item(&self) -> Option<&RecommendationItem> {
        let id = self.selected_recommendation?;
        self.recommendations
            .iter()
            .find(|item| item.article_id == id)
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
        if self.section == ShellSection::Recommendations {
            return format!("我的推荐 · {} 篇", self.recommendations.len());
        }
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

    /// 从数据源刷新“我的推荐”，并保留仍然存在的选中项。
    fn reload_recommendations(&mut self, data: &dyn ShellData) -> bool {
        let before = (
            self.recommendations.clone(),
            self.selected_recommendation,
            self.recommendations_error.clone(),
        );
        match data.recommendations() {
            Ok(recommendations) => {
                self.recommendations = recommendations;
                self.recommendations_error = None;
                self.selected_recommendation = self.selected_recommendation.filter(|id| {
                    self.recommendations
                        .iter()
                        .any(|item| item.article_id == *id)
                });
            }
            Err(error) => {
                self.recommendations.clear();
                self.selected_recommendation = None;
                self.recommendations_error = Some(error.to_string());
            }
        }
        before
            != (
                self.recommendations.clone(),
                self.selected_recommendation,
                self.recommendations_error.clone(),
            )
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
