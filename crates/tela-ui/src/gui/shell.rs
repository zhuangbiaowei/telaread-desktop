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
use crate::gui::theme::{ActiveTelaTheme as _, hsla, icon_element};
use crate::shell::{
    ArticleItem, BookmarkItem, BookmarkService, FeedItem, OfflineCacheProgress, OfflineCacheReport,
    OfflineCacheService, RecommendationItem, RecommendationService, ShellData, ShellSection,
    ShellViewModel,
};
use crate::theme::{TelaTheme, ThemeCatalog, ThemeColor};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// 左上角品牌文字点击后打开的 TelaRead 主页。
pub const HOME_URL: &str = "https://telaread.com";

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
    /// 左侧订阅列表的滚动句柄：`Shift+J/K` 切换订阅时让列表跟着滚。
    feed_scroll: ScrollHandle,
    /// 中间文章列表的滚动句柄：`j/k` 切换文章时让列表跟着滚。
    article_scroll: ScrollHandle,
    /// “我的推荐”列表的滚动句柄：`j/k` 切换推荐项时让列表跟着滚。
    recommendation_scroll: ScrollHandle,
    /// “我的收藏”列表的滚动句柄：`j/k` 切换收藏项时让列表跟着滚。
    bookmark_scroll: ScrollHandle,
    /// 可用主题 + 当前选择；主题选择器从这里取列表。
    themes: ThemeCatalog,
    /// 主题选择器是否打开（窗口内模态覆盖层）。
    theme_picker_open: bool,
    /// 一次主题操作的结果/错误提示。
    theme_notice: Option<String>,
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
        Self::new_with_theme_catalog(
            data,
            content,
            accounts,
            offline_cache,
            recommendations,
            bookmarks,
            ThemeCatalog::builtin(),
            cx,
        )
    }

    /// 完整装配 + 主题目录（真实 GUI）：构造时安装当前主题的全局值。
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_theme_catalog(
        data: Arc<dyn ShellData>,
        content: Arc<dyn ContentApi>,
        accounts: Option<Arc<dyn AccountService>>,
        offline_cache: Option<Arc<dyn OfflineCacheService>>,
        recommendations: Option<Arc<dyn RecommendationService>>,
        bookmarks: Option<Arc<dyn BookmarkService>>,
        themes: ThemeCatalog,
        cx: &mut Context<Self>,
    ) -> Self {
        // 直接构造视图（测试、其他宿主）时也保证全局主题与目录一致；
        // 真实入口在开窗前已经安装过，这里只是幂等地再设置一次。
        crate::gui::theme::install(cx, themes.active().clone());
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
            feed_scroll: ScrollHandle::new(),
            article_scroll: ScrollHandle::new(),
            recommendation_scroll: ScrollHandle::new(),
            bookmark_scroll: ScrollHandle::new(),
            themes,
            theme_picker_open: false,
            theme_notice: None,
            _reader_subscription,
        };
        view.sync_reader(cx);
        view
    }

    /// 当前视图模型（测试与状态栏用）。
    pub fn model(&self) -> &ShellViewModel {
        &self.model
    }

    /// 左侧订阅列表的滚动位置（测试/诊断用）。
    pub fn feed_scroll_offset(&self) -> Point<Pixels> {
        self.feed_scroll.offset()
    }

    /// 中间文章列表的滚动位置（测试/诊断用）。
    pub fn article_scroll_offset(&self) -> Point<Pixels> {
        self.article_scroll.offset()
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

    /// 当前主题目录/选择器数据（测试与诊断用）。
    pub fn themes(&self) -> &ThemeCatalog {
        &self.themes
    }

    /// 当前主题 id（测试用；渲染时以全局主题为准）。
    pub fn active_theme_id(&self) -> &str {
        self.themes.active_id()
    }

    /// 主题选择器是否打开。
    pub fn theme_picker_open(&self) -> bool {
        self.theme_picker_open
    }

    /// 最近一次主题操作的提示。
    pub fn theme_notice(&self) -> Option<&str> {
        self.theme_notice.as_deref()
    }

    /// 打开主题选择器；每次打开都重扫主题目录，方便边改文件边看效果。
    pub fn open_theme_picker(&mut self, cx: &mut Context<Self>) {
        self.themes.reload();
        self.sync_installed_theme(cx);
        self.theme_notice = None;
        self.theme_picker_open = true;
        // 遮罩层没有输入框，焦点仍在阅读面板：显式锁住它的 o/c/t 与翻页键。
        self.reader
            .update(cx, |reader, _| reader.set_input_locked(true));
        cx.notify();
    }

    /// 关闭主题选择器。
    pub fn close_theme_picker(&mut self, cx: &mut Context<Self>) {
        self.theme_picker_open = false;
        self.reader
            .update(cx, |reader, _| reader.set_input_locked(false));
        cx.notify();
    }

    /// 重新扫描本机主题目录。
    pub fn refresh_themes(&mut self, cx: &mut Context<Self>) {
        self.themes.reload();
        self.sync_installed_theme(cx);
        let count = self.themes.themes().len();
        self.theme_notice = Some(if self.themes.issues().is_empty() {
            format!("已重新扫描：共 {count} 个主题")
        } else {
            format!(
                "已重新扫描：共 {count} 个主题，{} 个文件有问题",
                self.themes.issues().len()
            )
        });
        cx.notify();
    }

    /// 主题目录重新扫描后，让全局主题回到目录里的当前选择。
    ///
    /// 覆盖两种情况：当前主题文件被删（退回默认主题）、主题内容被改
    /// （同 id 但颜色/字体变了）。
    fn sync_installed_theme(&mut self, cx: &mut Context<Self>) {
        let active = self.themes.active().clone();
        if *cx.tela_theme() == active {
            return;
        }
        crate::gui::theme::install(cx, active);
        self.reader.update(cx, |_, cx| cx.notify());
    }

    /// 应用一个主题：写全局、存本地选择、让阅读面板重绘。
    ///
    /// 返回是否成功；主题 id 不存在时保持当前主题并给出提示。
    pub fn apply_theme(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let Some(theme) = self.themes.select(id) else {
            self.theme_notice = Some(format!("找不到主题「{id}」"));
            cx.notify();
            return false;
        };
        let theme = theme.clone();
        crate::gui::theme::install(cx, theme.clone());
        self.theme_notice = match self.themes.persist_active() {
            Ok(()) => Some(format!("已切换到「{}」", theme.name)),
            Err(error) => Some(format!(
                "已切换到「{}」，但保存选择失败：{error}",
                theme.name
            )),
        };
        // 主题全局变了，阅读面板不会自动知道；显式让它重绘。
        self.reader.update(cx, |_, cx| cx.notify());
        cx.notify();
        true
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
            // 点击/快捷键切换后，让左侧订阅和中间文章列表跟上选中项。
            self.scroll_feed_into_view();
            self.scroll_to_selection();
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
        if changed {
            self.scroll_to_selection();
        }
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
            self.scroll_to_selection();
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
            self.scroll_to_selection();
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
        if changed {
            self.scroll_to_selection();
        }
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
        self.scroll_to_selection();
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
        self.scroll_to_selection();
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
        self.scroll_feed_into_view();
        self.scroll_to_selection();
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
        self.scroll_feed_into_view();
        self.scroll_to_selection();
        true
    }

    /// 把当前订阅滚进左侧列表的可视区；已可见时 GPUI 只做最小移动。
    fn scroll_feed_into_view(&self) {
        if let Some(index) = self
            .model
            .selected_feed
            .and_then(|id| self.model.feeds.iter().position(|feed| feed.id == id))
        {
            self.feed_scroll.scroll_to_item(index);
        }
    }

    /// 把当前栏的选中条目滚进列表的可视区。
    ///
    /// 列表在渲染时用 `track_scroll` 记录每个子项的位置，这里只登记目标下标，
    /// 真正的偏移在下一帧 prepaint 里生效；因此 `j/k` 走到屏幕外时列表会跟随，
    /// 仍在屏幕内时则不会乱跳。
    fn scroll_to_selection(&self) {
        match self.model.section {
            ShellSection::Feed => {
                if let Some(index) = self.model.selected_article.and_then(|id| {
                    self.model
                        .articles
                        .iter()
                        .position(|article| article.id == id)
                }) {
                    self.article_scroll.scroll_to_item(index);
                }
            }
            ShellSection::Recommendations => {
                if let Some(index) = self.model.selected_recommendation.and_then(|id| {
                    self.model
                        .recommendations
                        .iter()
                        .position(|item| item.article_id == id)
                }) {
                    self.recommendation_scroll.scroll_to_item(index);
                }
            }
            ShellSection::Bookmarks => {
                if let Some(index) = self
                    .model
                    .selected_bookmark
                    .and_then(|id| self.model.bookmarks.iter().position(|item| item.id == id))
                {
                    self.bookmark_scroll.scroll_to_item(index);
                }
            }
        }
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
        // 弹窗是模态的：输入网址/推荐语/选择主题时外壳不应抢 j/k。
        if self.theme_picker_open
            || self.bookmark_dialog.is_some()
            || self.reader.read(cx).recommend_dialog_open()
        {
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
    fn account_panel(&self, theme: &TelaTheme, cx: &mut Context<Self>) -> Div {
        let colors = &theme.colors;
        let mut panel = v_flex().gap_2().child(
            div()
                .text_xs()
                .text_color(hsla(colors.subtle))
                .child(self.account.label()),
        );

        if self.account.mode == AccountMode::Real && !self.account.client_id_configured {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(hsla(colors.warning))
                    .child("缺少 TELA_CLIENT_ID：设置环境变量后重启，或用 --demo 进入演示模式"),
            );
        }
        if let Some(error) = &self.account.store_error {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(hsla(colors.danger))
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
                                    .text_color(hsla(colors.muted))
                                    .truncate()
                                    .child(format!("库 {}", self.account.db_path)),
                            )
                            .child(
                                h_flex()
                                    .id("account-logout")
                                    .role(Role::Button)
                                    .aria_label("退出登录")
                                    .px_2()
                                    .py_1()
                                    .gap_1()
                                    .items_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .border_1()
                                    .border_color(hsla(colors.danger))
                                    .text_xs()
                                    .text_color(hsla(colors.danger))
                                    .hover(|style| {
                                        style
                                            .bg(hsla(colors.danger))
                                            .text_color(hsla(colors.background))
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| this.logout(cx)))
                                    .child(icon_element(
                                        &theme.icons.button,
                                        hsla(colors.danger),
                                        px(12.),
                                    ))
                                    .child("退出"),
                            ),
                    );
                } else if self.account.mode == AccountMode::Real {
                    panel = panel.child(
                        h_flex()
                            .id("account-login")
                            .role(Role::Button)
                            .aria_label("登录 Tela")
                            .px_2()
                            .py_1()
                            .gap_1()
                            .items_center()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(hsla(colors.accent))
                            .text_color(hsla(colors.on_accent))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .on_click(cx.listener(|this, _, _, cx| this.start_login(cx)))
                            .child(icon_element(
                                &theme.icons.button,
                                hsla(colors.on_accent),
                                px(12.),
                            ))
                            .child("登录 Tela"),
                    );
                }
            }
            LoginPhase::Waiting(ticket) => {
                panel = panel
                    .child(
                        div()
                            .text_xs()
                            .text_color(hsla(colors.accent))
                            .child("等待浏览器授权…"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(hsla(colors.muted))
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
                                    .bg(hsla(colors.accent))
                                    .text_color(hsla(colors.on_accent))
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
                                    .border_color(hsla(theme.borders.border))
                                    .text_xs()
                                    .text_color(hsla(colors.subtle))
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
                                    .text_color(hsla(colors.muted))
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_login(cx)))
                                    .child("取消"),
                            ),
                    );
            }
            LoginPhase::Error(error) => {
                panel = panel
                    .child(
                        div()
                            .text_xs()
                            .text_color(hsla(colors.danger))
                            .child(error.clone()),
                    )
                    .child(
                        h_flex()
                            .id("account-retry-login")
                            .role(Role::Button)
                            .aria_label("重试登录")
                            .px_2()
                            .py_1()
                            .gap_1()
                            .items_center()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(hsla(colors.accent))
                            .text_color(hsla(colors.on_accent))
                            .text_xs()
                            .on_click(cx.listener(|this, _, _, cx| this.start_login(cx)))
                            .child(icon_element(
                                &theme.icons.button,
                                hsla(colors.on_accent),
                                px(12.),
                            ))
                            .child("重试登录"),
                    );
            }
        }

        if let Some(notice) = &self.account_notice {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(hsla(colors.muted))
                    .child(notice.clone()),
            );
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

    fn sidebar(&self, theme: &TelaTheme, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        let colors = &theme.colors;
        v_flex()
            .w(px(260.))
            .h_full()
            .bg(hsla(colors.sidebar))
            .border_r_1()
            .border_color(hsla(theme.borders.border))
            .child(
                v_flex()
                    .p_4()
                    .gap_1()
                    .border_b_1()
                    .border_color(hsla(theme.borders.divider))
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
                                    .hover(|style| style.bg(hsla(colors.hover)))
                                    .on_click(cx.listener(|this, _, _, cx| this.open_home(cx)))
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("TelaRead"),
                                    )
                                    .child(icon_element(
                                        &theme.icons.external,
                                        hsla(colors.muted),
                                        px(12.),
                                    )),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(hsla(colors.muted))
                                    .child("本地优先阅读器"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(hsla(colors.subtle))
                            .child(model.status_line()),
                    )
                    .when_some(self.home_notice.clone(), |element, notice| {
                        element.child(
                            div()
                                .text_xs()
                                .text_color(hsla(colors.warning))
                                .child(notice),
                        )
                    })
                    .when_some(self.cache_notice.clone(), |element, notice| {
                        element.child(div().text_xs().text_color(hsla(colors.muted)).child(notice))
                    }),
            )
            .child(self.recommendations_entry(theme, cx))
            .child(self.bookmarks_entry(theme, cx))
            .child(self.feed_list(theme, cx))
            .child(
                v_flex()
                    .p_3()
                    .gap_1()
                    .border_t_1()
                    .border_color(hsla(theme.borders.divider))
                    .child(self.theme_entry(theme, cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(hsla(colors.muted))
                            .child(format!(
                                "{} 个订阅 · {} 篇未读",
                                model.feeds.len(),
                                model.unread_total()
                            )),
                    )
                    .child(self.account_panel(theme, cx)),
            )
    }

    /// 侧栏底部的主题入口：显示当前主题名，点击打开选择器。
    fn theme_entry(&self, theme: &TelaTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.themes.active();
        let label = format!("主题 · {}", current.name);
        h_flex()
            .id("theme-open")
            .role(Role::Button)
            .aria_label(format!("选择主题，当前 {}", current.name))
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            .justify_between()
            .rounded_md()
            .cursor_pointer()
            .border_1()
            .border_color(hsla(theme.borders.border))
            .hover(|style| style.bg(hsla(theme.colors.hover)))
            .on_click(cx.listener(|this, _, _, cx| this.open_theme_picker(cx)))
            .child(
                h_flex()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(icon_element(
                        &theme.icons.theme,
                        hsla(theme.colors.accent),
                        px(14.),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(hsla(theme.colors.subtle))
                            .truncate()
                            .child(label),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(hsla(theme.colors.muted))
                    .child("切换"),
            )
    }

    /// 侧栏的“我的推荐”入口：显示全部推荐过的文章数量。
    fn recommendations_entry(&self, theme: &TelaTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.model.section == ShellSection::Recommendations;
        let count = self.model.recommendations.len();
        let colors = &theme.colors;
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
            .when(selected, |element| element.bg(hsla(colors.selected)))
            .when(!selected, |element| {
                element.hover(|style| style.bg(hsla(colors.hover)))
            })
            .on_click(cx.listener(|this, _, _, cx| {
                if this.show_recommendations(cx) {
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(icon_element(
                        &theme.icons.recommendation,
                        hsla(if selected {
                            colors.accent
                        } else {
                            colors.muted
                        }),
                        px(14.),
                    ))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(if selected {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .child("我的推荐"),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(hsla(if selected {
                        colors.accent
                    } else {
                        colors.muted
                    }))
                    .child(count.to_string()),
            )
    }

    /// 侧栏的“我的收藏”入口：显示本地收藏数量。
    fn bookmarks_entry(&self, theme: &TelaTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.model.section == ShellSection::Bookmarks;
        let count = self.model.bookmarks.len();
        let colors = &theme.colors;
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
            .when(selected, |element| element.bg(hsla(colors.selected)))
            .when(!selected, |element| {
                element.hover(|style| style.bg(hsla(colors.hover)))
            })
            .on_click(cx.listener(|this, _, _, cx| {
                if this.show_bookmarks(cx) {
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(icon_element(
                        &theme.icons.bookmark,
                        hsla(if selected {
                            colors.accent
                        } else {
                            colors.muted
                        }),
                        px(14.),
                    ))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(if selected {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .child("我的收藏"),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(hsla(if selected {
                        colors.accent
                    } else {
                        colors.muted
                    }))
                    .child(count.to_string()),
            )
    }

    fn feed_list(&self, theme: &TelaTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let model = &self.model;
        let mut list = v_flex()
            .id("feed-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.feed_scroll)
            .p_2()
            .gap_1();

        if let Some(error) = &model.feeds_error {
            return list.child(Self::inline_error("订阅列表加载失败", error, theme));
        }
        if model.feeds.is_empty() {
            return list.child(Self::sidebar_empty(theme));
        }

        for feed in &model.feeds {
            list = list.child(self.feed_row(feed, model.selected_feed, theme, cx));
        }
        list
    }

    fn feed_row(
        &self,
        feed: &FeedItem,
        selected: Option<i64>,
        theme: &TelaTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let feed_id = feed.id;
        let colors = &theme.colors;
        let is_selected = self.model.section == ShellSection::Feed && selected == Some(feed.id);
        let cache_button = self.cache_button(feed, theme, cx);
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
            .when(is_selected, |element| element.bg(hsla(colors.selected)))
            .when(!is_selected, |element| {
                element.hover(|style| style.bg(hsla(colors.hover)))
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
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(icon_element(
                                &theme.icons.feed,
                                hsla(if is_selected {
                                    colors.accent
                                } else {
                                    colors.muted
                                }),
                                px(14.),
                            ))
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
                            ),
                    )
                    // 离线缓存按钮：博客名右侧，点一次缓存该订阅全部原文与译文。
                    .child(cache_button)
                    .when(feed.unread > 0, |element| {
                        element.child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(hsla(colors.accent))
                                .child(feed.unread.to_string()),
                        )
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(hsla(colors.muted))
                    .truncate()
                    .child(feed.feed_url.clone()),
            )
    }

    /// 博客名右侧的离线缓存按钮。
    fn cache_button(
        &self,
        feed: &FeedItem,
        theme: &TelaTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let feed_id = feed.id;
        let colors = &theme.colors;
        let progress = self.cache_progress.get(&feed_id);
        let running = progress.is_some_and(OfflineCacheProgress::is_running);
        let (label, color) = match progress {
            Some(progress) if !progress.finished => (progress.button_label(), colors.accent),
            Some(progress) if progress.failed > 0 => (progress.button_label(), colors.danger),
            Some(progress) if progress.pending > 0 => (progress.button_label(), colors.warning),
            Some(progress) => (progress.button_label(), colors.subtle),
            None => ("缓存".to_string(), colors.muted),
        };
        h_flex()
            .id(("feed-cache", feed_id as u64))
            .role(Role::Button)
            .aria_label(format!("离线缓存「{}」的原文与译文", feed.title))
            .px_1()
            .py_1()
            .gap_1()
            .items_center()
            .rounded_sm()
            .text_xs()
            .text_color(hsla(color))
            .when(running, |element| element.bg(hsla(colors.selected)))
            .when(!running, |element| {
                element.cursor_pointer().hover(|style| {
                    style
                        .bg(hsla(colors.selected))
                        .text_color(hsla(colors.accent))
                })
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                // 按钮嵌在整行“选中订阅”的可点击区域里，必须阻止事件冒泡，
                // 否则点缓存会顺带切换当前订阅。
                cx.stop_propagation();
                this.cache_feed(feed_id, cx);
            }))
            .child(icon_element(&theme.icons.cache, hsla(color), px(12.)))
            .child(label)
            .into_any_element()
    }

    fn sidebar_empty(theme: &TelaTheme) -> Div {
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
                    .text_color(hsla(theme.colors.muted))
                    .child("同步接入后，这里会出现订阅与文章。"),
            )
    }

    fn main_panel(&self, theme: &TelaTheme, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        let background = hsla(theme.colors.background);
        if model.section == ShellSection::Feed {
            if let Some(error) = &model.feeds_error {
                return h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .bg(background)
                    .child(Self::full_page_error("无法加载订阅", error, theme));
            }
            if model.is_empty() {
                return h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .bg(background)
                    .child(Self::full_page_empty(theme));
            }
        }

        let middle = match model.section {
            ShellSection::Feed => self.article_pane(theme, cx),
            ShellSection::Recommendations => self.recommendation_pane(theme, cx),
            ShellSection::Bookmarks => self.bookmark_pane(theme, cx),
        };
        h_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(background)
            .child(middle)
            .child(self.reader_pane())
    }

    fn article_pane(&self, theme: &TelaTheme, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        let colors = &theme.colors;
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
            .track_scroll(&self.article_scroll)
            .p_2()
            .gap_1();

        if let Some(error) = &model.articles_error {
            list = list.child(Self::inline_error("文章列表加载失败", error, theme));
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
                            .text_color(hsla(colors.muted))
                            .child("同步完成后，文章会出现在这里。"),
                    ),
            );
        } else {
            for article in &model.articles {
                list = list.child(Self::article_row(
                    article,
                    model.selected_article,
                    theme,
                    cx,
                ));
            }
        }

        v_flex()
            .w(px(340.))
            .h_full()
            .bg(hsla(colors.surface))
            .border_r_1()
            .border_color(hsla(theme.borders.border))
            .child(
                h_flex()
                    .p_4()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(hsla(theme.borders.divider))
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
                            .text_color(hsla(colors.muted))
                            .child(format!("{count} 篇")),
                    ),
            )
            .child(list)
    }

    /// 中间栏的“我的推荐”列表：标题、博客、推荐日期、推荐语与取消按钮。
    fn recommendation_pane(&self, theme: &TelaTheme, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        let colors = &theme.colors;
        let count = model.recommendations.len();

        let mut list = v_flex()
            .id("recommendation-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.recommendation_scroll)
            .p_2()
            .gap_1();

        if let Some(error) = &model.recommendations_error {
            list = list.child(Self::inline_error("推荐列表加载失败", error, theme));
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
                            .text_color(hsla(colors.muted))
                            .child("打开一篇文章，点右上角的「推荐」写下推荐理由。"),
                    ),
            );
        } else {
            for item in &model.recommendations {
                list = list.child(self.recommendation_row(item, theme, cx));
            }
        }

        v_flex()
            .w(px(360.))
            .h_full()
            .bg(hsla(colors.surface))
            .border_r_1()
            .border_color(hsla(theme.borders.border))
            .child(
                h_flex()
                    .p_4()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(hsla(theme.borders.divider))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("我的推荐"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(hsla(colors.muted))
                            .child(format!("{count} 篇")),
                    ),
            )
            .when_some(self.recommend_notice.clone(), |element, notice| {
                element.child(
                    div()
                        .px_4()
                        .pt_2()
                        .text_xs()
                        .text_color(hsla(colors.subtle))
                        .child(notice),
                )
            })
            .child(list)
    }

    fn recommendation_row(
        &self,
        item: &RecommendationItem,
        theme: &TelaTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let article_id = item.article_id;
        let colors = &theme.colors;
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
            .when(selected, |element| element.bg(hsla(colors.selected)))
            .when(!selected, |element| {
                element.hover(|style| style.bg(hsla(colors.sidebar)))
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
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(icon_element(
                                &theme.icons.recommendation,
                                hsla(if selected {
                                    colors.accent
                                } else {
                                    colors.muted
                                }),
                                px(12.),
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(item.title.clone()),
                            ),
                    )
                    .child(self.cancel_recommendation_button(article_id, theme, cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(hsla(colors.muted))
                    .truncate()
                    .child(format!("{} · {}", item.feed_title, item.created_label())),
            )
            .when_some(item.note.clone(), |element, note| {
                element.child(
                    div()
                        .text_xs()
                        .text_color(hsla(colors.subtle))
                        .child(format!("“{note}”")),
                )
            })
    }

    /// 推荐行右侧的“取消推荐”按钮；阻止事件冒泡，避免顺带打开文章。
    fn cancel_recommendation_button(
        &self,
        article_id: i64,
        theme: &TelaTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &theme.colors;
        h_flex()
            .id(("recommendation-cancel", article_id as u64))
            .role(Role::Button)
            .aria_label("取消推荐")
            .px_2()
            .py_1()
            .gap_1()
            .items_center()
            .rounded_md()
            .text_xs()
            .text_color(hsla(colors.danger))
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(hsla(colors.danger))
                    .text_color(hsla(colors.background))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.cancel_recommendation(article_id, cx);
            }))
            .child(icon_element(
                &theme.icons.remove,
                hsla(colors.danger),
                px(12.),
            ))
            .child("取消")
            .into_any_element()
    }

    /// 中间栏的“我的收藏”列表：标题、站点、收藏日期、图片数与移除按钮。
    fn bookmark_pane(&self, theme: &TelaTheme, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        let colors = &theme.colors;
        let count = model.bookmarks.len();

        let mut list = v_flex()
            .id("bookmark-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.bookmark_scroll)
            .p_2()
            .gap_1();

        if let Some(error) = &model.bookmarks_error {
            list = list.child(Self::inline_error("收藏列表加载失败", error, theme));
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
                            .text_color(hsla(colors.muted))
                            .child("按 Shift+B 输入一个网址，Tela 会下载正文、标题和图片。"),
                    ),
            );
        } else {
            for item in &model.bookmarks {
                list = list.child(self.bookmark_row(item, theme, cx));
            }
        }

        v_flex()
            .w(px(360.))
            .h_full()
            .bg(hsla(colors.surface))
            .border_r_1()
            .border_color(hsla(theme.borders.border))
            .child(
                h_flex()
                    .p_4()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(hsla(theme.borders.divider))
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
                                    .text_color(hsla(colors.muted))
                                    .child(format!("{count} 篇")),
                            )
                            // 除了 Shift+B，也给一个可见入口：新用户不必先记住快捷键。
                            .child(
                                h_flex()
                                    .id("bookmark-add")
                                    .role(Role::Button)
                                    .aria_label("收藏一个网址（Shift+B）")
                                    .px_2()
                                    .py_1()
                                    .gap_1()
                                    .items_center()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(hsla(theme.borders.border))
                                    .text_xs()
                                    .text_color(hsla(colors.subtle))
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style
                                            .bg(hsla(colors.selected))
                                            .text_color(hsla(colors.accent))
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_bookmark_dialog(window, cx);
                                    }))
                                    .child(icon_element(
                                        &theme.icons.add,
                                        hsla(colors.subtle),
                                        px(12.),
                                    ))
                                    .child("收藏"),
                            ),
                    ),
            )
            .when_some(self.bookmark_notice.clone(), |element, notice| {
                element.child(
                    div()
                        .px_4()
                        .pt_2()
                        .text_xs()
                        .text_color(hsla(colors.subtle))
                        .child(notice),
                )
            })
            .child(list)
    }

    fn bookmark_row(
        &self,
        item: &BookmarkItem,
        theme: &TelaTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bookmark_id = item.id;
        let colors = &theme.colors;
        let url = item.url.clone();
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
            .when(selected, |element| element.bg(hsla(colors.selected)))
            .when(!selected, |element| {
                element.hover(|style| style.bg(hsla(colors.sidebar)))
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
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(icon_element(
                                &theme.icons.bookmark,
                                hsla(if selected {
                                    colors.accent
                                } else {
                                    colors.muted
                                }),
                                px(12.),
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(item.title.clone()),
                            ),
                    )
                    .child(self.remove_bookmark_button(bookmark_id, theme, cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(hsla(colors.muted))
                    .truncate()
                    .child(item.meta_label()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(hsla(colors.muted))
                    .truncate()
                    .child(url),
            )
    }

    /// 收藏行右侧的“移除”按钮；阻止事件冒泡，避免顺带打开这篇。
    fn remove_bookmark_button(
        &self,
        bookmark_id: i64,
        theme: &TelaTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &theme.colors;
        h_flex()
            .id(("bookmark-remove", bookmark_id as u64))
            .role(Role::Button)
            .aria_label("从收藏中移除")
            .px_2()
            .py_1()
            .gap_1()
            .items_center()
            .rounded_md()
            .text_xs()
            .text_color(hsla(colors.danger))
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(hsla(colors.danger))
                    .text_color(hsla(colors.background))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.remove_bookmark(bookmark_id, cx);
            }))
            .child(icon_element(
                &theme.icons.remove,
                hsla(colors.danger),
                px(12.),
            ))
            .child("移除")
            .into_any_element()
    }

    fn article_row(
        article: &ArticleItem,
        selected: Option<i64>,
        theme: &TelaTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let article_id = article.id;
        let colors = &theme.colors;
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
            .when(is_selected, |element| element.bg(hsla(colors.selected)))
            .when(!is_selected, |element| {
                element.hover(|style| style.bg(hsla(colors.sidebar)))
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
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(icon_element(
                                &theme.icons.article,
                                hsla(if is_selected {
                                    colors.accent
                                } else {
                                    colors.muted
                                }),
                                px(12.),
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .when(article.unread, |element| {
                                        element.font_weight(FontWeight::SEMIBOLD)
                                    })
                                    .truncate()
                                    .child(article.title.clone()),
                            ),
                    )
                    .when(article.unread, |element| {
                        element.child(
                            div()
                                .text_xs()
                                .text_color(hsla(colors.accent))
                                .child("未读"),
                        )
                    }),
            )
            .when_some(article.published_label(), |element, published| {
                element.child(
                    div()
                        .text_xs()
                        .text_color(hsla(colors.muted))
                        .child(published),
                )
            })
            .when_some(article.excerpt.clone(), |element, excerpt| {
                element.child(
                    div()
                        .text_xs()
                        .text_color(hsla(colors.muted))
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

    fn inline_error(title: &str, detail: &str, theme: &TelaTheme) -> Div {
        v_flex()
            .p_3()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(hsla(theme.colors.danger))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(hsla(theme.colors.danger))
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(hsla(theme.colors.subtle))
                    .child(detail.to_string()),
            )
    }

    fn full_page_error(title: &str, detail: &str, theme: &TelaTheme) -> Div {
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
                    .text_color(hsla(theme.colors.danger))
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(hsla(theme.colors.subtle))
                    .child(detail.to_string()),
            )
    }

    fn full_page_empty(theme: &TelaTheme) -> Div {
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
                    .text_color(hsla(theme.colors.muted))
                    .child("这是 S2 的空态；接入同步后，这里会出现订阅列表。"),
            )
    }

    /// 主题选择器：一组图标/字体/颜色/边框就是一个主题。
    ///
    /// 是窗口内覆盖层而不是独立窗口：主题切换要立刻看到背后的三栏与正文变化。
    fn theme_picker_layer(&self, theme: &TelaTheme, cx: &mut Context<Self>) -> AnyElement {
        if !self.theme_picker_open {
            return div().into_any_element();
        }
        let colors = &theme.colors;
        let active_id = self.themes.active_id().to_string();
        let mut rows: Vec<AnyElement> = Vec::new();
        for candidate in self.themes.themes() {
            let id = candidate.id.clone();
            let click_id = id.clone();
            let active = id == active_id;
            let name = candidate.name.clone();
            let source = candidate.source.label();
            let appearance = candidate.appearance.label();
            let body_font = candidate.typography.body_font.clone();
            let swatch = |color: ThemeColor| {
                div()
                    .w(px(14.))
                    .h(px(14.))
                    .rounded_sm()
                    .border_1()
                    .border_color(hsla(theme.borders.border))
                    .bg(hsla(color))
            };
            rows.push(
                v_flex()
                    .id(format!("theme-{id}"))
                    .role(Role::Button)
                    .aria_label(format!("应用主题 {name}（{source}，{appearance}）"))
                    .min_w(px(240.))
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .cursor_pointer()
                    .border_1()
                    .border_color(hsla(if active {
                        theme.borders.focus
                    } else {
                        theme.borders.border
                    }))
                    .when(active, |element| element.bg(hsla(colors.selected)))
                    .when(!active, |element| {
                        element.hover(|style| style.bg(hsla(colors.hover)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.apply_theme(&click_id, cx);
                    }))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(hsla(colors.muted))
                                    .child(if active { "当前" } else { "应用" }),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(hsla(colors.muted))
                                    .child(format!("{appearance} · {source}")),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .child(swatch(candidate.colors.background))
                                    .child(swatch(candidate.colors.surface))
                                    .child(swatch(candidate.colors.text))
                                    .child(swatch(candidate.colors.accent))
                                    .child(swatch(candidate.borders.border)),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .text_color(hsla(colors.muted))
                                    .child(icon_element(
                                        &candidate.icons.feed,
                                        hsla(colors.muted),
                                        px(14.),
                                    ))
                                    .child(icon_element(
                                        &candidate.icons.bookmark,
                                        hsla(colors.muted),
                                        px(14.),
                                    ))
                                    .child(icon_element(
                                        &candidate.icons.article,
                                        hsla(colors.muted),
                                        px(14.),
                                    ))
                                    .child(icon_element(
                                        &candidate.icons.recommendation,
                                        hsla(colors.muted),
                                        px(14.),
                                    ))
                                    .child(icon_element(
                                        &candidate.icons.theme,
                                        hsla(colors.muted),
                                        px(14.),
                                    )),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(hsla(colors.subtle))
                                    .when_some(body_font, |element, font| element.font_family(font))
                                    .child("Aa 文"),
                            ),
                    )
                    .into_any_element(),
            );
        }

        let issues: Vec<String> = self
            .themes
            .issues()
            .iter()
            .map(ToString::to_string)
            .collect();
        let notice = self.theme_notice.clone();
        let dir_line = self
            .themes
            .themes_dir()
            .map(|dir| format!("主题目录：{}", dir.display()))
            .unwrap_or_else(|| "内置主题（未启用本机主题目录）".to_string());
        let has_dir = self.themes.themes_dir().is_some();
        let backdrop_close = cx.listener(|this, _, _, cx| this.close_theme_picker(cx));
        let panel_stop = cx.listener(|_, _, _, cx| cx.stop_propagation());
        let refresh = cx.listener(|this, _, _, cx| this.refresh_themes(cx));
        let close = cx.listener(|this, _, _, cx| this.close_theme_picker(cx));
        let copy_dir = cx.listener(|this, _, _, cx| {
            if let Some(dir) = this
                .themes
                .themes_dir()
                .map(|dir| dir.display().to_string())
            {
                cx.write_to_clipboard(ClipboardItem::new_string(dir.clone()));
                this.theme_notice = Some(format!("主题目录已复制：{dir}"));
                cx.notify();
            }
        });

        div()
            .id("theme-picker-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000066))
            .on_click(backdrop_close)
            .child(
                v_flex()
                    .id("theme-picker")
                    .w(px(640.))
                    .max_w_full()
                    .max_h(px(640.))
                    .m_4()
                    .p_5()
                    .gap_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(hsla(theme.borders.border))
                    .bg(hsla(colors.surface))
                    .text_color(hsla(colors.text))
                    .on_click(panel_stop)
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("主题"),
                            )
                            .child(
                                h_flex()
                                    .id("theme-close")
                                    .role(Role::Button)
                                    .aria_label("关闭主题选择")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(hsla(colors.muted))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(hsla(colors.selected)))
                                    .on_click(close)
                                    .child("关闭"),
                            ),
                    )
                    .child(div().text_xs().text_color(hsla(colors.muted)).child(
                        "图标、字体与颜色、边框修饰，一组就是一个主题；\
                                 主题文件放在本机主题目录，重启或「重新扫描」后可用。",
                    ))
                    .when_some(notice, |element, notice| {
                        element.child(
                            div()
                                .text_xs()
                                .text_color(hsla(colors.subtle))
                                .child(notice),
                        )
                    })
                    .child(
                        v_flex()
                            .id("theme-list")
                            .max_h(px(420.))
                            .overflow_y_scroll()
                            .gap_2()
                            .children(rows),
                    )
                    .when(!issues.is_empty(), |element| {
                        let count = issues.len();
                        element.child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(hsla(colors.warning))
                                        .child(format!("{count} 个主题文件未加载：")),
                                )
                                .children(issues.into_iter().take(5).map(|issue| {
                                    div().text_xs().text_color(hsla(colors.danger)).child(issue)
                                })),
                        )
                    })
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(hsla(colors.muted))
                                    .truncate()
                                    .child(dir_line),
                            )
                            .child(h_flex().items_center().gap_2().when(has_dir, |element| {
                                element
                                    .child(
                                        h_flex()
                                            .id("theme-copy-dir")
                                            .role(Role::Button)
                                            .aria_label("复制主题目录")
                                            .px_2()
                                            .py_1()
                                            .gap_1()
                                            .items_center()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(hsla(theme.borders.border))
                                            .text_xs()
                                            .text_color(hsla(colors.subtle))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(hsla(colors.selected)))
                                            .on_click(copy_dir)
                                            .child("复制目录"),
                                    )
                                    .child(
                                        h_flex()
                                            .id("theme-refresh")
                                            .role(Role::Button)
                                            .aria_label("重新扫描主题目录")
                                            .px_2()
                                            .py_1()
                                            .gap_1()
                                            .items_center()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(hsla(theme.borders.border))
                                            .text_xs()
                                            .text_color(hsla(colors.subtle))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(hsla(colors.selected)))
                                            .on_click(refresh)
                                            .child(icon_element(
                                                &theme.icons.button,
                                                hsla(colors.subtle),
                                                px(12.),
                                            ))
                                            .child("重新扫描"),
                                    )
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for ShellView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 主题按帧读取：切换后外壳、阅读面板与选择器看到同一份数据。
        let theme = cx.tela_theme();
        let sidebar = self.sidebar(&theme, cx);
        let main = self.main_panel(&theme, cx);
        let picker = self.theme_picker_layer(&theme, cx);
        div()
            .id("shell-root")
            .relative()
            .size_full()
            .bg(hsla(theme.colors.background))
            .text_color(hsla(theme.colors.text))
            .when_some(theme.typography.ui_font.clone(), |element, font| {
                element.font_family(font)
            })
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // 主题选择器是窗口内模态：Esc 关闭，其他快捷键一律不抢。
                if this.theme_picker_open {
                    if event.keystroke.key.eq_ignore_ascii_case("escape") {
                        this.close_theme_picker(cx);
                    }
                    return;
                }
                let modifiers = event.keystroke.modifiers;
                if modifiers.control || modifiers.alt || modifiers.platform {
                    return;
                }
                if this.handle_shortcut(&event.keystroke.key, modifiers.shift, window, cx) {
                    cx.notify();
                }
            }))
            .child(h_flex().size_full().child(sidebar).child(main))
            .when_some(self.bookmark_dialog.clone(), |element, dialog| {
                element.child(dialog)
            })
            .child(picker)
    }
}
