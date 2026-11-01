//! 三栏外壳的 GPUI 视图（S2）。
//!
//! 只做“把 [`ShellViewModel`] 画成元素 + 把点击转成模型操作”，
//! 不查库、不发 HTTP、不解析正文。阅读面板在 S3 由 [`crate::gui::reader`] 接管。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tela_content::api::ContentApi;
use tela_proto::TelaError;

use crate::account::{AccountMode, AccountService, AccountStatus, LoginPhase, LoginTicket};
use crate::gui::bookmark::{BookmarkDialogEvent, BookmarkDialogView};
use crate::gui::reader::{ReaderEvent, ReaderView};
use crate::shell::{
    ArticleItem, BookmarkItem, BookmarkService, FeedItem, OfflineCacheProgress, OfflineCacheReport,
    OfflineCacheService, RecommendationItem, RecommendationService, ShellData, ShellSection,
    ShellViewModel,
};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// 左上角品牌文字点击后打开的 TelaRead 主页。
pub const HOME_URL: &str = "https://telaread.com";

const BG: u32 = 0x11111b;
const SIDEBAR: u32 = 0x181825;
const PANEL: u32 = 0x1e1e2e;
const BORDER: u32 = 0x313244;
const TEXT: u32 = 0xcdd6f4;
const MUTED: u32 = 0x6c7086;
const SUBTLE: u32 = 0xa6adc8;
const ACCENT: u32 = 0x89b4fa;
const DANGER: u32 = 0xf38ba8;
const WARN: u32 = 0xf9e2af;

/// 三栏外壳视图：左订阅、中文章、右阅读面板。
pub struct ShellView {
    data: Arc<dyn ShellData>,
    content: Arc<dyn ContentApi>,
    model: ShellViewModel,
    reader: Entity<ReaderView>,
    accounts: Option<Arc<dyn AccountService>>,
    account: AccountStatus,
    login: LoginPhase,
    account_notice: Option<String>,
    /// 打开 TelaRead 主页失败时的提示（成功不打扰用户）。
    home_notice: Option<String>,
    login_generation: u64,
    login_task: Option<Task<()>>,
    sync_in_flight: bool,
    sync_again: bool,
    sync_task: Option<Task<()>>,
    /// 订阅级离线缓存能力；测试/无账号装配可以为空（按钮仍显示，点击给出提示）。
    offline_cache: Option<Arc<dyn OfflineCacheService>>,
    /// 每个订阅最近一次缓存进度（左侧按钮显示它）。
    cache_progress: HashMap<i64, OfflineCacheProgress>,
    /// 缓存完成/失败的一句提示（侧栏顶部）。
    cache_notice: Option<String>,
    /// 后台缓存任务与进度轮询任务，必须保活。
    cache_tasks: Vec<Task<()>>,
    /// 推荐写能力；测试/只读装配可以为空（推荐按钮不显示）。
    recommendations: Option<Arc<dyn RecommendationService>>,
    /// 推荐列表上的一句提示（完成/失败）。
    recommend_notice: Option<String>,
    /// 网页收藏写能力；测试/只读装配可以为空（收藏弹窗会给出提示）。
    bookmarks: Option<Arc<dyn BookmarkService>>,
    /// 收藏网页弹窗；`None` 表示关闭。弹窗自己负责下载与错误提示。
    bookmark_dialog: Option<Entity<BookmarkDialogView>>,
    /// 收藏成功/失败的一句提示（侧栏顶部与收藏列表）。
    bookmark_notice: Option<String>,
    /// 收藏弹窗的事件订阅；必须保活。
    bookmark_subscription: Option<Subscription>,
    /// 阅读面板的推荐事件订阅；必须保活。
    _reader_subscription: Subscription,
}

impl ShellView {
    /// 装配视图并做首次加载（同步；mock 数据在微秒级，真实库也是本地 SQLite）。
    pub fn new(
        data: Arc<dyn ShellData>,
        content: Arc<dyn ContentApi>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_accounts(data, content, None, cx)
    }

    /// 带 Login/Logout 的装配。
    pub fn new_with_accounts(
        data: Arc<dyn ShellData>,
        content: Arc<dyn ContentApi>,
        accounts: Option<Arc<dyn AccountService>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_services(data, content, accounts, None, cx)
    }

    /// 完整装配：账号能力 + 订阅级离线缓存能力（真实 GUI 用）。
    pub fn new_with_services(
        data: Arc<dyn ShellData>,
        content: Arc<dyn ContentApi>,
        accounts: Option<Arc<dyn AccountService>>,
        offline_cache: Option<Arc<dyn OfflineCacheService>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_recommendations(data, content, accounts, offline_cache, None, cx)
    }

    /// 完整装配：账号、离线缓存与推荐写能力。
    pub fn new_with_recommendations(
        data: Arc<dyn ShellData>,
        content: Arc<dyn ContentApi>,
        accounts: Option<Arc<dyn AccountService>>,
        offline_cache: Option<Arc<dyn OfflineCacheService>>,
        recommendations: Option<Arc<dyn RecommendationService>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_bookmarks(
            data,
            content,
            accounts,
            offline_cache,
            recommendations,
            None,
            cx,
        )
    }

    /// 完整装配：账号、离线缓存、推荐与网页收藏（`Shift+B`）。
    pub fn new_with_bookmarks(
        data: Arc<dyn ShellData>,
        content: Arc<dyn ContentApi>,
        accounts: Option<Arc<dyn AccountService>>,
        offline_cache: Option<Arc<dyn OfflineCacheService>>,
        recommendations: Option<Arc<dyn RecommendationService>>,
        bookmarks: Option<Arc<dyn BookmarkService>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let reader = cx.new(|cx| {
            ReaderView::new_with_recommendations(
                data.clone(),
                content.clone(),
                recommendations.clone(),
                cx,
            )
        });
        let mut model = ShellViewModel::new();
        model.reload(data.as_ref());
        let account = accounts
            .as_ref()
            .map(|accounts| accounts.status())
            .unwrap_or_default();
        let _reader_subscription =
            cx.subscribe(&reader, |this, _reader, _event: &ReaderEvent, cx| {
                // 阅读面板已经在本地写入了推荐行；外壳只负责刷新列表与补一次同步。
                this.reload(cx);
                this.schedule_sync(cx);
                cx.notify();
            });
        let mut view = Self {
            data,
            content,
            model,
            reader,
            accounts,
            account,
            login: LoginPhase::Idle,
            account_notice: None,
            home_notice: None,
            login_generation: 0,
            login_task: None,
            sync_in_flight: false,
            sync_again: false,
            sync_task: None,
            offline_cache,
            cache_progress: HashMap::new(),
            cache_notice: None,
            cache_tasks: Vec::new(),
            recommendations,
            recommend_notice: None,
            bookmarks,
            bookmark_dialog: None,
            bookmark_notice: None,
            bookmark_subscription: None,
            _reader_subscription,
        };
        view.sync_reader(cx);
        view
    }

    /// 当前视图模型（测试与状态栏用）。
    pub fn model(&self) -> &ShellViewModel {
        &self.model
    }

    /// 阅读面板实体（测试/后续状态联动用）。
    pub fn reader(&self) -> Entity<ReaderView> {
        self.reader.clone()
    }

    /// 当前账号快照。
    pub fn account(&self) -> &AccountStatus {
        &self.account
    }

    /// 当前登录阶段。
    pub fn login_phase(&self) -> &LoginPhase {
        &self.login
    }

    /// 内容 API 句柄（诊断/后续设置页用）。
    pub fn content(&self) -> Arc<dyn ContentApi> {
        Arc::clone(&self.content)
    }

    /// 打开 TelaRead 主页失败时的提示（测试/诊断用）。
    pub fn home_notice(&self) -> Option<&str> {
        self.home_notice.as_deref()
    }

    /// 用系统浏览器打开 TelaRead 主页（左上角品牌文字）。
    pub fn open_home(&mut self, cx: &mut Context<Self>) {
        match tela_net::browser::open(HOME_URL) {
            Ok(()) => self.home_notice = None,
            Err(error) => self.home_notice = Some(format!("打开 TelaRead 主页失败：{error}")),
        }
        cx.notify();
    }

    /// 某个订阅当前的离线缓存进度（测试/按钮文案用）。
    pub fn cache_progress(&self, feed_id: i64) -> Option<&OfflineCacheProgress> {
        self.cache_progress.get(&feed_id)
    }

    /// 最近一次缓存完成/失败的提示。
    pub fn cache_notice(&self) -> Option<&str> {
        self.cache_notice.as_deref()
    }

    /// 点击博客名旁的“缓存”按钮：后台缓存该订阅所有未缓存的原文与译文。
    ///
    /// 同一订阅正在缓存时重复点击会被忽略；完成后可以再点一次补齐仍在
    /// 生成中的译文（服务端翻译是异步的）。
    pub fn cache_feed(&mut self, feed_id: i64, cx: &mut Context<Self>) {
        if self
            .cache_progress
            .get(&feed_id)
            .is_some_and(OfflineCacheProgress::is_running)
        {
            return;
        }
        let Some(cache) = self.offline_cache.clone() else {
            self.cache_notice = Some("当前装配不支持离线缓存".to_string());
            cx.notify();
            return;
        };
        let title = self
            .model
            .feeds
            .iter()
            .find(|feed| feed.id == feed_id)
            .map(|feed| feed.title.clone())
            .unwrap_or_else(|| format!("订阅 {feed_id}"));

        let progress = Arc::new(Mutex::new(OfflineCacheProgress {
            feed_id,
            ..OfflineCacheProgress::default()
        }));
        let outcome: Arc<Mutex<Option<Result<OfflineCacheReport, TelaError>>>> =
            Arc::new(Mutex::new(None));
        self.cache_progress.insert(
            feed_id,
            progress.lock().map(|p| p.clone()).unwrap_or_default(),
        );
        self.cache_notice = Some(format!("开始离线缓存「{title}」…"));

        // 后台线程做网络/磁盘；进度回调只写共享快照，不直接碰 UI。
        let background_progress = Arc::clone(&progress);
        let background_outcome = Arc::clone(&outcome);
        let background_cache = Arc::clone(&cache);
        let callback_progress = Arc::clone(&background_progress);
        let background_task = cx.background_executor().spawn(async move {
            let result = background_cache.cache_feed(feed_id, &move |snapshot| {
                if let Ok(mut guard) = callback_progress.lock() {
                    *guard = snapshot;
                }
            });
            if let Ok(mut slot) = background_outcome.lock() {
                *slot = Some(result);
            }
            if let Ok(mut guard) = background_progress.lock() {
                guard.finished = true;
            }
        });
        self.cache_tasks.push(background_task);

        // 前台定时把进度搬到视图并重绘；后台任务结束后收尾一次。
        let poll_progress = Arc::clone(&progress);
        let poll_outcome = Arc::clone(&outcome);
        let poll_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let snapshot = poll_progress
                    .lock()
                    .map(|guard| guard.clone())
                    .unwrap_or_default();
                let finished = snapshot.finished;
                let result = poll_outcome.lock().ok().and_then(|slot| slot.clone());
                let keep_going = this.update(cx, |view, cx| {
                    view.cache_progress.insert(feed_id, snapshot.clone());
                    if finished {
                        view.finish_cache(feed_id, &title, result);
                    }
                    cx.notify();
                    !finished
                });
                if !matches!(keep_going, Ok(true)) {
                    break;
                }
            }
        });
        self.cache_tasks.push(poll_task);
        // 任务句柄只增不减是有意的：中途 drain 会取消仍在缓存的任务。
        // 每次点击只产生两个句柄，一次会话内的数量很小。
        cx.notify();
    }

    /// 缓存收尾：把最终结果写进提示，并把进度快照标记为完成。
    fn finish_cache(
        &mut self,
        feed_id: i64,
        title: &str,
        result: Option<Result<OfflineCacheReport, TelaError>>,
    ) {
        match result {
            Some(Ok(report)) => {
                let detail = report
                    .errors
                    .first()
                    .map(|error| format!("；{error}"))
                    .unwrap_or_default();
                self.cache_notice = Some(format!(
                    "「{title}」离线缓存完成：{}{detail}",
                    report.summary()
                ));
                let mut progress = self
                    .cache_progress
                    .get(&feed_id)
                    .cloned()
                    .unwrap_or_default();
                progress.finished = true;
                progress.originals = report.originals_cached;
                progress.translations = report.translations_cached;
                progress.images = report.images_cached;
                progress.images_failed = report.images_failed;
                progress.skipped = report.skipped;
                progress.failed = report.failed;
                progress.pending = report.pending;
                self.cache_progress.insert(feed_id, progress);
            }
            Some(Err(error)) => {
                self.cache_notice = Some(format!("「{title}」离线缓存失败：{error}"));
                if let Some(progress) = self.cache_progress.get_mut(&feed_id) {
                    progress.finished = true;
                    progress.failed += 1;
                }
            }
            None => {
                self.cache_notice = Some(format!("「{title}」离线缓存任务结束，但没有拿到结果"));
            }
        }
    }

    /// 选中订阅；返回是否发生变化。
    pub fn select_feed(&mut self, feed_id: i64, cx: &mut Context<Self>) -> bool {
        let changed = self.model.select_feed(feed_id, self.data.as_ref());
        if changed {
            self.recommend_notice = None;
            self.sync_reader(cx);
            // 切订阅会显示该订阅的第一篇，视同打开。
            if let Some(article_id) = self.model.selected_article {
                let _ = self.mark_read_if_needed(article_id, cx);
            }
        }
        changed
    }

    /// 打开“我的推荐”栏；返回是否发生变化。
    pub fn show_recommendations(&mut self, cx: &mut Context<Self>) -> bool {
        let changed = self.model.show_recommendations(self.data.as_ref());
        if changed {
            self.recommend_notice = None;
            // 回到推荐栏时，如果之前选过一条，就把阅读面板切回那篇；
            // 没选过则保持当前阅读内容，不自动打开推荐（避免误标已读）。
            self.sync_reader(cx);
            cx.notify();
        }
        changed
    }

    /// 选中一条推荐并交给阅读面板；返回是否发生变化。
    pub fn select_recommendation(&mut self, article_id: i64, cx: &mut Context<Self>) -> bool {
        let changed = self.model.select_recommendation(article_id);
        if changed {
            self.sync_reader(cx);
        }
        let marked = self.mark_read_if_needed(article_id, cx);
        changed || marked
    }

    /// 从推荐列表撤回一条推荐；本地立即生效，后台补一次同步。
    pub fn cancel_recommendation(&mut self, article_id: i64, cx: &mut Context<Self>) {
        let Some(service) = self.recommendations.clone() else {
            self.recommend_notice = Some("当前装配不支持推荐".to_string());
            cx.notify();
            return;
        };
        match service.unrecommend(article_id) {
            Ok(()) => {
                self.model.reload(self.data.as_ref());
                self.reader.update(cx, |reader, cx| {
                    reader.refresh_recommendation(cx);
                });
                self.recommend_notice = Some("已取消推荐。".to_string());
                self.schedule_sync(cx);
            }
            Err(error) => {
                self.recommend_notice = Some(format!("取消推荐失败：{error}"));
            }
        }
        cx.notify();
    }

    /// 最近一次推荐操作的结果提示（测试/诊断用）。
    pub fn recommend_notice(&self) -> Option<&str> {
        self.recommend_notice.as_deref()
    }

    /// 最近一次收藏操作的结果提示（测试/诊断用）。
    pub fn bookmark_notice(&self) -> Option<&str> {
        self.bookmark_notice.as_deref()
    }

    /// 收藏弹窗是否打开（测试/诊断用）。
    pub fn bookmark_dialog_open(&self) -> bool {
        self.bookmark_dialog.is_some()
    }

    /// 收藏弹窗实体（测试/诊断用；关闭时为 `None`）。
    pub fn bookmark_dialog(&self) -> Option<Entity<BookmarkDialogView>> {
        self.bookmark_dialog.clone()
    }

    /// 打开收藏网页弹窗（`Shift+B`）。
    ///
    /// 推荐弹窗已经打开时不抢焦点：两个模态同时出现会让 Esc/Enter 的归属变得
    /// 不可预测。没有收藏能力（只读装配）时也给一句明确的提示。
    pub fn open_bookmark_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bookmark_dialog.is_some() {
            return;
        }
        if self.reader.read(cx).recommend_dialog_open() {
            return;
        }
        if self.bookmarks.is_none() {
            self.bookmark_notice = Some("当前装配不支持收藏网页".to_string());
            cx.notify();
            return;
        }

        let service = self.bookmarks.clone();
        let dialog = cx.new(|cx| BookmarkDialogView::new(service, window, cx));
        let subscription = cx.subscribe(
            &dialog,
            |this, _dialog, event: &BookmarkDialogEvent, cx| match event {
                BookmarkDialogEvent::Saved(item) => this.bookmark_saved(item, cx),
                BookmarkDialogEvent::Cancelled => {
                    this.bookmark_dialog = None;
                    cx.notify();
                }
            },
        );
        self.bookmark_dialog = Some(dialog);
        self.bookmark_subscription = Some(subscription);
        self.bookmark_notice = None;
        cx.notify();
    }

    /// 关闭收藏弹窗（保存成功或用户取消）。
    pub fn close_bookmark_dialog(&mut self, cx: &mut Context<Self>) {
        self.bookmark_dialog = None;
        self.bookmark_subscription = None;
        cx.notify();
    }

    /// 收藏成功：关闭弹窗、刷新「我的收藏」，正在看收藏列表时顺带打开新收藏。
    fn bookmark_saved(&mut self, item: &BookmarkItem, cx: &mut Context<Self>) {
        self.bookmark_dialog = None;
        self.bookmark_subscription = None;
        self.model.reload(self.data.as_ref());
        self.bookmark_notice = Some(format!("已收藏「{}」到「我的收藏」", item.title));
        if self.model.section == ShellSection::Bookmarks {
            self.model.select_bookmark(item.id);
            self.sync_reader(cx);
        }
        cx.notify();
    }

    /// 打开“我的收藏”栏；返回是否发生变化。
    pub fn show_bookmarks(&mut self, cx: &mut Context<Self>) -> bool {
        let changed = self.model.show_bookmarks(self.data.as_ref());
        if changed {
            self.bookmark_notice = None;
            self.sync_reader(cx);
            cx.notify();
        }
        changed
    }

    /// 选中一条收藏并交给阅读面板；返回是否发生变化。
    pub fn select_bookmark(&mut self, bookmark_id: i64, cx: &mut Context<Self>) -> bool {
        let changed = self.model.select_bookmark(bookmark_id);
        if changed {
            self.sync_reader(cx);
        }
        changed
    }

    /// 从「我的收藏」里删除一条；本地立即生效（收藏不参与同步）。
    pub fn remove_bookmark(&mut self, bookmark_id: i64, cx: &mut Context<Self>) {
        let Some(service) = self.bookmarks.clone() else {
            self.bookmark_notice = Some("当前装配不支持收藏网页".to_string());
            cx.notify();
            return;
        };
        match service.remove_bookmark(bookmark_id) {
            Ok(()) => {
                let was_selected = self.model.selected_bookmark == Some(bookmark_id);
                self.model.reload(self.data.as_ref());
                self.bookmark_notice = Some("已从收藏中移除。".to_string());
                if was_selected {
                    // 删掉的正是正在读的这篇：阅读面板退回空态，避免继续显示已删正文。
                    self.reader.update(cx, |reader, cx| {
                        reader.clear_selection(cx);
                    });
                    self.sync_reader(cx);
                }
            }
            Err(error) => {
                self.bookmark_notice = Some(format!("移除收藏失败：{error}"));
            }
        }
        cx.notify();
    }

    /// 选中文章；返回是否发生变化。
    pub fn select_article(&mut self, article_id: i64, cx: &mut Context<Self>) -> bool {
        let changed = self.model.select_article(article_id);
        if changed {
            self.sync_reader(cx);
        }
        // 即使这篇已经选中（例如启动时自动展示的第一篇），再点一次也应当标已读。
        let marked = self.mark_read_if_needed(article_id, cx);
        changed || marked
    }

    /// 下一篇（`j`）：停在当前订阅的文章列表末尾。
    pub fn next_article(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.model.next_article() {
            return false;
        }
        self.sync_reader(cx);
        if let Some(article_id) = self.model.selected_article {
            let _ = self.mark_read_if_needed(article_id, cx);
        }
        true
    }

    /// 上一篇（`k`）：停在当前订阅的文章列表开头。
    pub fn prev_article(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.model.prev_article() {
            return false;
        }
        self.sync_reader(cx);
        if let Some(article_id) = self.model.selected_article {
            let _ = self.mark_read_if_needed(article_id, cx);
        }
        true
    }

    /// 下一个博客（`Shift+J`）：切换订阅并打开它的第一篇文章。
    pub fn next_feed(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.model.next_feed(self.data.as_ref()) {
            return false;
        }
        self.sync_reader(cx);
        if let Some(article_id) = self.model.selected_article {
            let _ = self.mark_read_if_needed(article_id, cx);
        }
        true
    }

    /// 上一个博客（`Shift+K`）。
    pub fn prev_feed(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.model.prev_feed(self.data.as_ref()) {
            return false;
        }
        self.sync_reader(cx);
        if let Some(article_id) = self.model.selected_article {
            let _ = self.mark_read_if_needed(article_id, cx);
        }
        true
    }

    /// 外壳级快捷键：`j/k` 文章，`Shift+j/k` 博客，`Shift+b` 收藏网页。
    ///
    /// 阅读面板自己处理 `o/c/t/PageUp/PageDown/↑/↓`；这里只接管需要改
    /// 选中项或开弹窗的键，事件从阅读面板冒泡上来后调用。
    pub fn handle_shortcut(
        &mut self,
        key: &str,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // 弹窗是模态的：输入网址/推荐语时外壳不应抢 j/k。
        if self.bookmark_dialog.is_some() || self.reader.read(cx).recommend_dialog_open() {
            return false;
        }
        match (key.to_ascii_lowercase().as_str(), shift) {
            ("j", false) => self.next_article(cx),
            ("k", false) => self.prev_article(cx),
            ("j", true) => self.next_feed(cx),
            ("k", true) => self.prev_feed(cx),
            ("b", true) => {
                self.open_bookmark_dialog(window, cx);
                true
            }
            _ => false,
        }
    }

    /// 重新从数据源刷新（后台同步完成后调用）。
    pub fn reload(&mut self, cx: &mut Context<Self>) -> bool {
        let changed = self.model.reload(self.data.as_ref());
        if changed {
            self.sync_reader(cx);
            // 同步/登录切换后，当前文章的“已推荐”标记也要跟着本地行更新。
            self.reader.update(cx, |reader, cx| {
                reader.refresh_recommendation(cx);
            });
        }
        changed
    }

    /// 把当前选中的文章交给阅读面板；同一篇重复调用由阅读面板自己忽略。
    fn sync_reader(&mut self, cx: &mut Context<Self>) {
        let selection = match self.model.section {
            ShellSection::Feed => self
                .model
                .selected_article_item()
                .map(|article| (article.id, article.title.clone(), article.url.clone())),
            ShellSection::Recommendations => self
                .model
                .selected_recommendation_item()
                .map(|item| (item.article_id, item.title.clone(), item.url.clone())),
            ShellSection::Bookmarks => None,
        };
        if self.model.section == ShellSection::Bookmarks {
            if let Some(item) = self.model.selected_bookmark_item() {
                let (bookmark_id, title, url) =
                    (item.id, item.title.clone(), Some(item.url.clone()));
                self.reader.update(cx, |reader, cx| {
                    reader.select_bookmark(bookmark_id, title, url, cx)
                });
            }
            return;
        }
        if let Some((article_id, title, url)) = selection {
            self.reader
                .update(cx, |reader, cx| reader.select(article_id, title, url, cx));
        }
    }

    /// 打开文章即已读（尊重 `reader.mark_on_open`）：本地先变，再后台推送。
    fn mark_read_if_needed(&mut self, article_id: i64, cx: &mut Context<Self>) -> bool {
        if self.account.mode != AccountMode::Real {
            return false;
        }
        // read 令牌无法写：不要每点一篇文章弹一次 403，等用户重新授权为 write。
        if self.account.scope.as_deref() != Some("write") {
            return false;
        }
        let Some(accounts) = self.accounts.clone() else {
            return false;
        };
        match accounts.mark_read(article_id) {
            Ok(true) => {
                // 立刻刷列表：未读计数/加粗在当前帧就消失，不等网络。
                self.reload(cx);
                self.schedule_sync(cx);
                true
            }
            Ok(false) => false,
            Err(error) => {
                self.account_notice = Some(format!("标记已读失败：{error}"));
                cx.notify();
                false
            }
        }
    }

    /// 合并短时间内的多次同步请求：一次在飞时只记一个“再来一轮”。
    fn schedule_sync(&mut self, cx: &mut Context<Self>) {
        let Some(accounts) = self.accounts.clone() else {
            return;
        };
        if self.sync_in_flight {
            self.sync_again = true;
            return;
        }
        self.sync_in_flight = true;
        let generation = self.login_generation;
        let task = cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = cx
                .background_executor()
                .spawn(async move { accounts.sync_now() })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.login_generation != generation {
                    return;
                }
                view.sync_in_flight = false;
                if let Err(error) = result {
                    view.account_notice = Some(format!("本地变更已入队，但同步失败：{error}"));
                }
                view.reload(cx);
                if view.sync_again {
                    view.sync_again = false;
                    view.schedule_sync(cx);
                }
                cx.notify();
            });
        });
        self.sync_task = Some(task);
    }

    /// 侧栏底部的账号面板：登录 / 等待授权 / 退出。
    fn account_panel(&self, cx: &mut Context<Self>) -> Div {
        let mut panel = v_flex().gap_2().child(
            div()
                .text_xs()
                .text_color(rgb(SUBTLE))
                .child(self.account.label()),
        );

        if self.account.mode == AccountMode::Real && !self.account.client_id_configured {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(rgb(WARN))
                    .child("缺少 TELA_CLIENT_ID：设置环境变量后重启，或用 --demo 进入演示模式"),
            );
        }
        if let Some(error) = &self.account.store_error {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(rgb(DANGER))
                    .child(format!("凭据库：{error}")),
            );
        }

        match &self.login {
            LoginPhase::Idle => {
                if self.account.can_logout() {
                    panel = panel.child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .truncate()
                                    .child(format!("库 {}", self.account.db_path)),
                            )
                            .child(
                                div()
                                    .id("account-logout")
                                    .role(Role::Button)
                                    .aria_label("退出登录")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .border_1()
                                    .border_color(rgb(DANGER))
                                    .text_xs()
                                    .text_color(rgb(DANGER))
                                    .hover(|style| style.bg(rgb(DANGER)).text_color(rgb(BG)))
                                    .on_click(cx.listener(|this, _, _, cx| this.logout(cx)))
                                    .child("退出"),
                            ),
                    );
                } else if self.account.mode == AccountMode::Real {
                    panel = panel.child(
                        div()
                            .id("account-login")
                            .role(Role::Button)
                            .aria_label("登录 Tela")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(ACCENT))
                            .text_color(rgb(BG))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .on_click(cx.listener(|this, _, _, cx| this.start_login(cx)))
                            .child("登录 Tela"),
                    );
                }
            }
            LoginPhase::Waiting(ticket) => {
                panel = panel
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(ACCENT))
                            .child("等待浏览器授权…"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .truncate()
                            .child(ticket.auth_url.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .id("account-open-auth")
                                    .role(Role::Button)
                                    .aria_label("打开授权页")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(rgb(ACCENT))
                                    .text_color(rgb(BG))
                                    .text_xs()
                                    .on_click(cx.listener(|this, _, _, cx| this.open_auth_page(cx)))
                                    .child("打开"),
                            )
                            .child(
                                div()
                                    .id("account-copy-auth")
                                    .role(Role::Button)
                                    .aria_label("复制授权链接")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .text_xs()
                                    .text_color(rgb(SUBTLE))
                                    .on_click(cx.listener(|this, _, _, cx| this.copy_auth_link(cx)))
                                    .child("复制链接"),
                            )
                            .child(
                                div()
                                    .id("account-cancel-login")
                                    .role(Role::Button)
                                    .aria_label("取消登录")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_login(cx)))
                                    .child("取消"),
                            ),
                    );
            }
            LoginPhase::Error(error) => {
                panel = panel
                    .child(div().text_xs().text_color(rgb(DANGER)).child(error.clone()))
                    .child(
                        div()
                            .id("account-retry-login")
                            .role(Role::Button)
                            .aria_label("重试登录")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(ACCENT))
                            .text_color(rgb(BG))
                            .text_xs()
                            .on_click(cx.listener(|this, _, _, cx| this.start_login(cx)))
                            .child("重试登录"),
                    );
            }
        }

        if let Some(notice) = &self.account_notice {
            panel = panel.child(div().text_xs().text_color(rgb(MUTED)).child(notice.clone()));
        }
        panel
    }

    /// 点击「登录」：开端口、拿 URL、尝试开浏览器，并把等待放到后台。
    pub fn start_login(&mut self, cx: &mut Context<Self>) {
        let Some(accounts) = self.accounts.clone() else {
            self.login = LoginPhase::Error("当前是 demo 装配，没有账号能力".into());
            self.account_notice = None;
            cx.notify();
            return;
        };
        self.login_generation = self.login_generation.wrapping_add(1);
        let generation = self.login_generation;
        match accounts.begin_login() {
            Ok(ticket) => {
                // 终端里也打印一份：浏览器打不开时用户可直接复制。
                eprintln!("tela-gui: 授权地址 {}", ticket.auth_url);
                let open_error = accounts.open_auth_url(&ticket).err();
                self.account_notice = Some(match open_error {
                    Some(error) => {
                        format!("浏览器未打开：{error}；链接已打印到终端，可点「复制链接」")
                    }
                    None => "已在浏览器打开授权页，等待授权…".to_string(),
                });
                self.login = LoginPhase::Waiting(ticket.clone());
                self.spawn_login(accounts, ticket, generation, cx);
            }
            Err(error) => {
                self.login = LoginPhase::Error(error.to_string());
                self.account_notice = None;
            }
        }
        cx.notify();
    }

    fn spawn_login(
        &mut self,
        accounts: Arc<dyn AccountService>,
        ticket: LoginTicket,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result: Result<(AccountStatus, Result<(), TelaError>), TelaError> = cx
                .background_executor()
                .spawn(async move {
                    let status = accounts.wait_login(&ticket)?;
                    let sync = accounts.sync_now();
                    Ok((status, sync))
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                if view.login_generation != generation {
                    return;
                }
                match result {
                    Ok((status, sync)) => {
                        view.account = status;
                        view.login = LoginPhase::Idle;
                        view.account_notice = sync
                            .err()
                            .map(|error| format!("登录成功，但同步失败：{error}"));
                        view.reload(cx);
                    }
                    Err(error) => {
                        view.login = LoginPhase::Error(error.to_string());
                        view.account_notice = None;
                    }
                }
                cx.notify();
            });
        });
        self.login_task = Some(task);
    }

    /// 重新打开授权页（浏览器第一次没起来时）。
    pub fn open_auth_page(&mut self, cx: &mut Context<Self>) {
        let Some(ticket) = self.login.ticket().cloned() else {
            return;
        };
        let Some(accounts) = self.accounts.clone() else {
            return;
        };
        match accounts.open_auth_url(&ticket) {
            Ok(()) => self.account_notice = Some("已请求打开浏览器".to_string()),
            Err(error) => {
                self.account_notice = Some(format!("仍无法打开浏览器：{error}；可点「复制链接」"))
            }
        }
        cx.notify();
    }

    /// 把授权链接放进剪贴板。
    pub fn copy_auth_link(&mut self, cx: &mut Context<Self>) {
        let Some(ticket) = self.login.ticket().cloned() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(ticket.auth_url));
        self.account_notice = Some("授权链接已复制到剪贴板".to_string());
        cx.notify();
    }

    /// 取消登录：关闭回环端口，并让迟到的后台结果失效。
    pub fn cancel_login(&mut self, cx: &mut Context<Self>) {
        if let Some(accounts) = self.accounts.clone() {
            accounts.cancel_login();
        }
        self.login_generation = self.login_generation.wrapping_add(1);
        self.login = LoginPhase::Idle;
        self.account_notice = Some("已取消登录".to_string());
        cx.notify();
    }

    /// 退出登录：撤销令牌、清本地账号数据、刷新列表。
    pub fn logout(&mut self, cx: &mut Context<Self>) {
        let Some(accounts) = self.accounts.clone() else {
            return;
        };
        self.login_generation = self.login_generation.wrapping_add(1);
        self.login = LoginPhase::Idle;
        self.sync_in_flight = false;
        self.sync_again = false;
        self.sync_task = None;
        match accounts.logout() {
            Ok(status) => {
                self.account = status;
                self.account_notice = Some("已退出登录".to_string());
                self.reload(cx);
            }
            Err(error) => {
                // 本地凭据通常已经被清掉，状态以服务端/本地真实值为准。
                self.account = accounts.status();
                self.account_notice = Some(format!("退出时出错：{error}"));
            }
        }
        cx.notify();
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        v_flex()
            .w(px(260.))
            .h_full()
            .bg(rgb(SIDEBAR))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                v_flex()
                    .p_4()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .id("home-link")
                                    .role(Role::Button)
                                    .aria_label("打开 TelaRead 主页")
                                    .items_center()
                                    .gap_1()
                                    .px_1()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(PANEL)))
                                    .on_click(cx.listener(|this, _, _, cx| this.open_home(cx)))
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("TelaRead"),
                                    )
                                    .child(div().text_xs().text_color(rgb(MUTED)).child("↗")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child("本地优先阅读器"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(SUBTLE))
                            .child(model.status_line()),
                    )
                    .when_some(self.home_notice.clone(), |element, notice| {
                        element.child(div().text_xs().text_color(rgb(WARN)).child(notice))
                    })
                    .when_some(self.cache_notice.clone(), |element, notice| {
                        element.child(div().text_xs().text_color(rgb(MUTED)).child(notice))
                    }),
            )
            .child(self.recommendations_entry(cx))
            .child(self.bookmarks_entry(cx))
            .child(self.feed_list(cx))
            .child(
                v_flex()
                    .p_3()
                    .gap_1()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                        "{} 个订阅 · {} 篇未读",
                        model.feeds.len(),
                        model.unread_total()
                    )))
                    .child(self.account_panel(cx)),
            )
    }

    /// 侧栏的“我的推荐”入口：显示全部推荐过的文章数量。
    fn recommendations_entry(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.model.section == ShellSection::Recommendations;
        let count = self.model.recommendations.len();
        h_flex()
            .id("sidebar-recommendations")
            .role(Role::Button)
            .aria_label(format!("我的推荐，{count} 篇"))
            .mx_2()
            .mt_2()
            .mb_1()
            .px_3()
            .py_2()
            .gap_2()
            .items_center()
            .justify_between()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |element| element.bg(rgb(BORDER)))
            .when(!selected, |element| {
                element.hover(|style| style.bg(rgb(PANEL)))
            })
            .on_click(cx.listener(|this, _, _, cx| {
                if this.show_recommendations(cx) {
                    cx.notify();
                }
            }))
            .child(
                div()
                    .text_sm()
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .child("我的推荐"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(if selected { ACCENT } else { MUTED }))
                    .child(count.to_string()),
            )
    }

    /// 侧栏的“我的收藏”入口：显示本地收藏数量。
    fn bookmarks_entry(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.model.section == ShellSection::Bookmarks;
        let count = self.model.bookmarks.len();
        h_flex()
            .id("sidebar-bookmarks")
            .role(Role::Button)
            .aria_label(format!("我的收藏，{count} 篇"))
            .mx_2()
            .mb_1()
            .px_3()
            .py_2()
            .gap_2()
            .items_center()
            .justify_between()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |element| element.bg(rgb(BORDER)))
            .when(!selected, |element| {
                element.hover(|style| style.bg(rgb(PANEL)))
            })
            .on_click(cx.listener(|this, _, _, cx| {
                if this.show_bookmarks(cx) {
                    cx.notify();
                }
            }))
            .child(
                div()
                    .text_sm()
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .child("我的收藏"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(if selected { ACCENT } else { MUTED }))
                    .child(count.to_string()),
            )
    }

    fn feed_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = &self.model;
        let mut list = v_flex()
            .id("feed-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_2()
            .gap_1();

        if let Some(error) = &model.feeds_error {
            return list.child(Self::inline_error("订阅列表加载失败", error));
        }
        if model.feeds.is_empty() {
            return list.child(Self::sidebar_empty());
        }

        for feed in &model.feeds {
            list = list.child(self.feed_row(feed, model.selected_feed, cx));
        }
        list
    }

    fn feed_row(
        &self,
        feed: &FeedItem,
        selected: Option<i64>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let feed_id = feed.id;
        let is_selected = self.model.section == ShellSection::Feed && selected == Some(feed.id);
        let cache_button = self.cache_button(feed, cx);
        v_flex()
            .id(("feed", feed.id as u64))
            .role(Role::Button)
            .aria_label(feed.title.clone())
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .when(is_selected, |element| element.bg(rgb(BORDER)))
            .when(!is_selected, |element| {
                element.hover(|style| style.bg(rgb(PANEL)))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.select_feed(feed_id, cx) {
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(if is_selected {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .truncate()
                            .child(feed.title.clone()),
                    )
                    // 离线缓存按钮：博客名右侧，点一次缓存该订阅全部原文与译文。
                    .child(cache_button)
                    .when(feed.unread > 0, |element| {
                        element.child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(ACCENT))
                                .child(feed.unread.to_string()),
                        )
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .truncate()
                    .child(feed.feed_url.clone()),
            )
    }

    /// 博客名右侧的离线缓存按钮。
    fn cache_button(&self, feed: &FeedItem, cx: &mut Context<Self>) -> AnyElement {
        let feed_id = feed.id;
        let progress = self.cache_progress.get(&feed_id);
        let running = progress.is_some_and(OfflineCacheProgress::is_running);
        let (label, color) = match progress {
            Some(progress) if !progress.finished => (progress.button_label(), ACCENT),
            Some(progress) if progress.failed > 0 => (progress.button_label(), DANGER),
            Some(progress) if progress.pending > 0 => (progress.button_label(), WARN),
            Some(progress) => (progress.button_label(), SUBTLE),
            None => ("缓存".to_string(), MUTED),
        };
        div()
            .id(("feed-cache", feed_id as u64))
            .role(Role::Button)
            .aria_label(format!("离线缓存「{}」的原文与译文", feed.title))
            .px_1()
            .py_1()
            .rounded_sm()
            .text_xs()
            .text_color(rgb(color))
            .when(running, |element| element.bg(rgb(BORDER)))
            .when(!running, |element| {
                element
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(BORDER)).text_color(rgb(ACCENT)))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                // 按钮嵌在整行“选中订阅”的可点击区域里，必须阻止事件冒泡，
                // 否则点缓存会顺带切换当前订阅。
                cx.stop_propagation();
                this.cache_feed(feed_id, cx);
            }))
            .child(label)
            .into_any_element()
    }

    fn sidebar_empty() -> Div {
        v_flex()
            .p_4()
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child("还没有订阅"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("同步接入后，这里会出现订阅与文章。"),
            )
    }

    fn main_panel(&self, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        if model.section == ShellSection::Feed {
            if let Some(error) = &model.feeds_error {
                return h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .bg(rgb(BG))
                    .child(Self::full_page_error("无法加载订阅", error));
            }
            if model.is_empty() {
                return h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .bg(rgb(BG))
                    .child(Self::full_page_empty());
            }
        }

        let middle = match model.section {
            ShellSection::Feed => self.article_pane(cx),
            ShellSection::Recommendations => self.recommendation_pane(cx),
            ShellSection::Bookmarks => self.bookmark_pane(cx),
        };
        h_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(rgb(BG))
            .child(middle)
            .child(self.reader_pane())
    }

    fn article_pane(&self, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        let title = model
            .selected_feed_item()
            .map(|feed| feed.title.clone())
            .unwrap_or_else(|| "文章".to_string());
        let count = model.articles.len();

        let mut list = v_flex()
            .id("article-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_2()
            .gap_1();

        if let Some(error) = &model.articles_error {
            list = list.child(Self::inline_error("文章列表加载失败", error));
        } else if model.articles.is_empty() {
            list = list.child(
                v_flex()
                    .p_4()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().child("这个订阅还没有文章"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("同步完成后，文章会出现在这里。"),
                    ),
            );
        } else {
            for article in &model.articles {
                list = list.child(Self::article_row(article, model.selected_article, cx));
            }
        }

        v_flex()
            .w(px(340.))
            .h_full()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                h_flex()
                    .p_4()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("{count} 篇")),
                    ),
            )
            .child(list)
    }

    /// 中间栏的“我的推荐”列表：标题、博客、推荐日期、推荐语与取消按钮。
    fn recommendation_pane(&self, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        let count = model.recommendations.len();

        let mut list = v_flex()
            .id("recommendation-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_2()
            .gap_1();

        if let Some(error) = &model.recommendations_error {
            list = list.child(Self::inline_error("推荐列表加载失败", error));
        } else if model.recommendations.is_empty() {
            list = list.child(
                v_flex()
                    .p_4()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().child("还没有推荐过文章"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("打开一篇文章，点右上角的「推荐」写下推荐理由。"),
                    ),
            );
        } else {
            for item in &model.recommendations {
                list = list.child(self.recommendation_row(item, cx));
            }
        }

        v_flex()
            .w(px(360.))
            .h_full()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                h_flex()
                    .p_4()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("我的推荐"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("{count} 篇")),
                    ),
            )
            .when_some(self.recommend_notice.clone(), |element, notice| {
                element.child(
                    div()
                        .px_4()
                        .pt_2()
                        .text_xs()
                        .text_color(rgb(SUBTLE))
                        .child(notice),
                )
            })
            .child(list)
    }

    fn recommendation_row(
        &self,
        item: &RecommendationItem,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let article_id = item.article_id;
        let selected = self.model.section == ShellSection::Recommendations
            && self.model.selected_recommendation == Some(article_id);
        v_flex()
            .id(("recommendation", article_id as u64))
            .role(Role::Button)
            .aria_label(item.title.clone())
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |element| element.bg(rgb(BORDER)))
            .when(!selected, |element| {
                element.hover(|style| style.bg(rgb(SIDEBAR)))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.select_recommendation(article_id, cx) {
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(item.title.clone()),
                    )
                    .child(self.cancel_recommendation_button(article_id, cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .truncate()
                    .child(format!("{} · {}", item.feed_title, item.created_label())),
            )
            .when_some(item.note.clone(), |element, note| {
                element.child(
                    div()
                        .text_xs()
                        .text_color(rgb(SUBTLE))
                        .child(format!("“{note}”")),
                )
            })
    }

    /// 推荐行右侧的“取消推荐”按钮；阻止事件冒泡，避免顺带打开文章。
    fn cancel_recommendation_button(&self, article_id: i64, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id(("recommendation-cancel", article_id as u64))
            .role(Role::Button)
            .aria_label("取消推荐")
            .px_2()
            .py_1()
            .rounded_md()
            .text_xs()
            .text_color(rgb(DANGER))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(DANGER)).text_color(rgb(BG)))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.cancel_recommendation(article_id, cx);
            }))
            .child("取消")
            .into_any_element()
    }

    /// 中间栏的“我的收藏”列表：标题、站点、收藏日期、图片数与移除按钮。
    fn bookmark_pane(&self, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        let count = model.bookmarks.len();

        let mut list = v_flex()
            .id("bookmark-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_2()
            .gap_1();

        if let Some(error) = &model.bookmarks_error {
            list = list.child(Self::inline_error("收藏列表加载失败", error));
        } else if model.bookmarks.is_empty() {
            list = list.child(
                v_flex()
                    .p_4()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().child("还没有收藏网页"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("按 Shift+B 输入一个网址，Tela 会下载正文、标题和图片。"),
                    ),
            );
        } else {
            for item in &model.bookmarks {
                list = list.child(self.bookmark_row(item, cx));
            }
        }

        v_flex()
            .w(px(360.))
            .h_full()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                h_flex()
                    .p_4()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("我的收藏"),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child(format!("{count} 篇")),
                            )
                            // 除了 Shift+B，也给一个可见入口：新用户不必先记住快捷键。
                            .child(
                                div()
                                    .id("bookmark-add")
                                    .role(Role::Button)
                                    .aria_label("收藏一个网址（Shift+B）")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .text_xs()
                                    .text_color(rgb(SUBTLE))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(BORDER)).text_color(rgb(ACCENT)))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_bookmark_dialog(window, cx);
                                    }))
                                    .child("＋ 收藏"),
                            ),
                    ),
            )
            .when_some(self.bookmark_notice.clone(), |element, notice| {
                element.child(
                    div()
                        .px_4()
                        .pt_2()
                        .text_xs()
                        .text_color(rgb(SUBTLE))
                        .child(notice),
                )
            })
            .child(list)
    }

    fn bookmark_row(&self, item: &BookmarkItem, cx: &mut Context<Self>) -> impl IntoElement {
        let bookmark_id = item.id;
        let selected = self.model.section == ShellSection::Bookmarks
            && self.model.selected_bookmark == Some(bookmark_id);
        v_flex()
            .id(("bookmark", bookmark_id as u64))
            .role(Role::Button)
            .aria_label(item.title.clone())
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |element| element.bg(rgb(BORDER)))
            .when(!selected, |element| {
                element.hover(|style| style.bg(rgb(SIDEBAR)))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.select_bookmark(bookmark_id, cx) {
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(item.title.clone()),
                    )
                    .child(self.remove_bookmark_button(bookmark_id, cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .truncate()
                    .child(item.meta_label()),
            )
            .when_some(Some(item.url.clone()), |element, url| {
                element.child(div().text_xs().text_color(rgb(MUTED)).truncate().child(url))
            })
    }

    /// 收藏行右侧的“移除”按钮；阻止事件冒泡，避免顺带打开这篇。
    fn remove_bookmark_button(&self, bookmark_id: i64, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id(("bookmark-remove", bookmark_id as u64))
            .role(Role::Button)
            .aria_label("从收藏中移除")
            .px_2()
            .py_1()
            .rounded_md()
            .text_xs()
            .text_color(rgb(DANGER))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(DANGER)).text_color(rgb(BG)))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.remove_bookmark(bookmark_id, cx);
            }))
            .child("移除")
            .into_any_element()
    }

    fn article_row(
        article: &ArticleItem,
        selected: Option<i64>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let article_id = article.id;
        let is_selected = selected == Some(article.id);
        v_flex()
            .id(("article", article.id as u64))
            .role(Role::Button)
            .aria_label(article.title.clone())
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .when(is_selected, |element| element.bg(rgb(BORDER)))
            .when(!is_selected, |element| {
                element.hover(|style| style.bg(rgb(SIDEBAR)))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.select_article(article_id, cx) {
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .when(article.unread, |element| {
                                element.font_weight(FontWeight::SEMIBOLD)
                            })
                            .truncate()
                            .child(article.title.clone()),
                    )
                    .when(article.unread, |element| {
                        element.child(div().text_xs().text_color(rgb(ACCENT)).child("未读"))
                    }),
            )
            .when_some(article.published_label(), |element, published| {
                element.child(div().text_xs().text_color(rgb(MUTED)).child(published))
            })
            .when_some(article.excerpt.clone(), |element, excerpt| {
                element.child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .truncate()
                        .child(excerpt),
                )
            })
    }

    fn reader_pane(&self) -> Div {
        // 阅读面板是独立实体：它自己管理异步加载/失败重试，外壳只负责选文章。
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.reader.clone())
    }

    fn inline_error(title: &str, detail: &str) -> Div {
        v_flex()
            .p_3()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(0xf38ba8))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xf38ba8))
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(SUBTLE))
                    .child(detail.to_string()),
            )
    }

    fn full_page_error(title: &str, detail: &str) -> Div {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .p_8()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xf38ba8))
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(SUBTLE))
                    .child(detail.to_string()),
            )
    }

    fn full_page_empty() -> Div {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .p_8()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("还没有订阅"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("这是 S2 的空态；接入同步后，这里会出现订阅列表。"),
            )
    }
}

impl Render for ShellView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.sidebar(cx);
        let main = self.main_panel(cx);
        h_flex()
            .id("shell-root")
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let modifiers = event.keystroke.modifiers;
                if modifiers.control || modifiers.alt || modifiers.platform {
                    return;
                }
                if this.handle_shortcut(&event.keystroke.key, modifiers.shift, window, cx) {
                    cx.notify();
                }
            }))
            .child(sidebar)
            .child(main)
            .when_some(self.bookmark_dialog.clone(), |element, dialog| {
                element.child(dialog)
            })
    }
}
