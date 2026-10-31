//! 本地优先的内容阅读层：正文与译本第一次从远端取回后写入本地库，
//! 后续打开只读本地；左侧博客列表的「离线缓存」按钮通过这里批量预取。
//!
//! 设计要点：
//!
//! - **正文不可变**：第一次下载后永久复用（`bodies` 表，由 `Store` 维护）；
//! - **译本按 `(contentKey, lang)` 快照**：`done/partial` 直接本地命中；
//!   `pending/none` 也会缓存，离线打开时仍能显示原文并给出状态；
//! - **图片按 URL 缓存原始字节**：第一次显示后落库，离线阅读不再请求图片代理；
//! - **轮询要绕过缓存**：等待服务端翻译时用 [`ContentApi::refresh_chapter`]，
//!   只重新查询译文状态，不重复下载正文；
//! - **自动离线判定**：任何远端请求返回 [`TelaError::Net`] 就把状态标为离线，
//!   阅读面板据此暂停译文轮询；离线期间只由调用方按低频做轻量探活（`/meta`），
//!   恢复后自动继续；
//! - **批量缓存顺序执行**：避免一次点击把令牌配额/带宽打满，进度逐篇回调。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tela_content::api::{ChapterRequest, ContentApi};
use tela_content::http::normalize_content_key;
use tela_content::model::{
    Chapter, ContentObject, TranslationAnswer, TranslationAnswerStatus, TranslationObject,
    TranslationRow, TranslationState, TranslationView,
};
use tela_proto::{ArticleRow, StoreStage, TelaError, WireContext};
use tela_store::Store;
use tela_ui::reader::document::image_sources_in_html;
use tela_ui::shell::{OfflineCacheProgress, OfflineCacheReport, OfflineCacheService};

/// 批量缓存时等待译文生成的轮询轮数（约十几秒；没完成的等下次点击继续）。
const PREFETCH_POLL_ROUNDS: usize = 10;
/// 批量缓存的轮询间隔。
const PREFETCH_POLL_INTERVAL: Duration = Duration::from_millis(1200);
/// 一次缓存操作里保留的失败提示条数。
const MAX_REPORT_ERRORS: usize = 8;
/// 「没有译文」的缓存状态（不等于 `failed`：作者可能关闭了翻译）。
pub const TRANSLATION_STATE_NONE: &str = "none";

/// 带本地缓存的 [`ContentApi`]。
///
/// `inner` 是真正的远端后端（HTTP 或 mock）；本层负责读写 SQLite，
/// 并根据请求结果自动维护「是否离线」——完全离线时阅读面板不再发译文轮询。
pub struct CachingContentApi {
    inner: Arc<dyn ContentApi>,
    store: Arc<Mutex<Store>>,
    /// 批量缓存默认使用的译文语言（单篇加载以 `ChapterRequest.lang` 为准）。
    lang: String,
    db_path: PathBuf,
    /// 最近一次网络请求是否失败；只在内存里，重启后重新探测。
    offline: AtomicBool,
}

impl CachingContentApi {
    /// 装配：`store` 与 `inner` 都由调用方共享。
    pub fn new(
        inner: Arc<dyn ContentApi>,
        store: Arc<Mutex<Store>>,
        lang: impl Into<String>,
        db_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            inner,
            store,
            lang: lang.into(),
            db_path: db_path.into(),
            offline: AtomicBool::new(false),
        }
    }

    /// 目标译文语言。
    pub fn lang(&self) -> &str {
        &self.lang
    }

    // ---- 本地缓存读写 ----

    fn lock_store(&self) -> Result<MutexGuard<'_, Store>, TelaError> {
        self.store.lock().map_err(|_| TelaError::Store {
            stage: StoreStage::Query,
            path: self.db_path.display().to_string(),
        })
    }

    // ---- 联网状态 ----

    /// 记录一次远端请求的结果：只有 `Net` 才算离线，其它错误说明能到达服务端。
    fn track<T>(&self, result: Result<T, TelaError>) -> Result<T, TelaError> {
        match &result {
            Ok(_) => self.offline.store(false, Ordering::SeqCst),
            Err(TelaError::Net { .. }) => self.offline.store(true, Ordering::SeqCst),
            Err(_) => self.offline.store(false, Ordering::SeqCst),
        }
        result
    }

    fn offline_error(&self) -> TelaError {
        TelaError::Net {
            ctx: WireContext::new("offline://network", "network"),
            detail: "网络连接不可用：已暂停译文状态请求，恢复后会自动继续".into(),
        }
    }

    /// 真正发一次轻量探活；调用方控制频率。
    fn probe_network_impl(&self) -> Result<(), TelaError> {
        if !self.is_offline() {
            return Ok(());
        }
        match self.track(self.inner.probe_network()) {
            Ok(()) => {
                self.offline.store(false, Ordering::SeqCst);
                Ok(())
            }
            // 网络仍不可用：保持离线，调用方稍后再试。
            Err(error @ TelaError::Net { .. }) => Err(error),
            // 能拿到服务端响应（哪怕是 5xx/限流）就说明网络已恢复。
            Err(_) => {
                self.offline.store(false, Ordering::SeqCst);
                Ok(())
            }
        }
    }

    /// 网络请求的离线闸门：离线时先探活一次，仍不通就快速失败、不发正文/译文/图片请求。
    fn ensure_online(&self) -> Result<(), TelaError> {
        if !self.is_offline() {
            return Ok(());
        }
        self.probe_network_impl()?;
        if self.is_offline() {
            return Err(self.offline_error());
        }
        Ok(())
    }

    fn fetch_chapter(&self, request: &ChapterRequest) -> Result<Chapter, TelaError> {
        self.track(self.inner.load_chapter(request))
    }

    fn fetch_translation_view(
        &self,
        content: &ContentObject,
        lang: &str,
    ) -> Result<TranslationView, TelaError> {
        self.track(self.inner.translation_view(content, lang))
    }

    // ---- 图片缓存 ----

    fn cached_image(&self, src: &str) -> Option<Vec<u8>> {
        self.lock_store().ok()?.get_image(src).ok()?
    }

    fn cache_image(&self, src: &str, body: &[u8]) -> Result<(), TelaError> {
        self.lock_store()?.put_image(src, body)
    }

    /// 下载并落库一张图片；已有缓存由调用方先查。
    fn fetch_image(&self, src: &str) -> Result<Vec<u8>, TelaError> {
        self.ensure_online()?;
        let body = self.track(self.inner.load_image(src))?;
        self.cache_image(src, &body)?;
        Ok(body)
    }

    /// 正文 + 已对齐译文里出现的全部图片地址（与阅读面板的 `image_sources` 同源）。
    fn image_sources_for(content: &ContentObject, translation: &TranslationView) -> Vec<String> {
        let mut sources = Vec::new();
        let mut push = |html: &str| {
            for src in image_sources_in_html(html) {
                if !sources.iter().any(|item| item == &src) {
                    sources.push(src);
                }
            }
        };
        for block in &content.blocks {
            push(&block.html);
        }
        if let Some(aligned) = translation.aligned() {
            for html in aligned.iter().flatten() {
                push(html);
            }
        }
        sources
    }

    /// 缓存一篇文章引用的图片；离线/网络失败会记录到报告，剩余图片留到下次点击。
    fn cache_article_images(
        &self,
        content: &ContentObject,
        translation: &TranslationView,
        article_id: i64,
        report: &mut OfflineCacheReport,
        snapshot: &mut OfflineCacheProgress,
        error_keys: &mut HashSet<String>,
    ) -> Result<(), TelaError> {
        for src in Self::image_sources_for(content, translation) {
            if self.cached_image(&src).is_some() {
                continue;
            }
            if self.is_offline() {
                report.images_failed += 1;
                snapshot.images_failed += 1;
                let message = format!("文章 {article_id}：网络已断开，剩余图片未缓存");
                if error_keys.insert(message.clone()) {
                    report.errors.push(message);
                    report.errors.truncate(MAX_REPORT_ERRORS);
                }
                return Ok(());
            }
            match self.fetch_image(&src) {
                Ok(_) => {
                    report.images_cached += 1;
                    snapshot.images += 1;
                }
                Err(error) => {
                    report.images_failed += 1;
                    snapshot.images_failed += 1;
                    record_error(report, error_keys, &error, article_id);
                    if self.is_offline() {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    /// 读缓存的正文对象；损坏的缓存当作未命中（下一次会重新下载）。
    fn cached_body(&self, content_key: &str) -> Option<ContentObject> {
        let body = self.lock_store().ok()?.get_body(content_key).ok()??;
        serde_json::from_str(&body).ok()
    }

    /// 写入正文对象（不可变，覆盖写也安全）。
    fn cache_body(&self, content: &ContentObject) -> Result<(), TelaError> {
        let body = serde_json::to_string(content).map_err(|error| TelaError::Config {
            detail: format!("正文对象无法序列化：{error}"),
        })?;
        self.lock_store()?.put_body(&content.key, &body)
    }

    /// 读缓存的译文视图：`done/partial` 返回 `Ready`，其余返回 pending/none。
    fn cached_translation_view(
        &self,
        content: &ContentObject,
        lang: &str,
    ) -> Option<TranslationView> {
        let cached = self
            .lock_store()
            .ok()?
            .get_translation(&content.key, lang)
            .ok()??;
        let state = TranslationState::parse(&cached.state);
        // 有对象就按 Ready 处理（open enum 的服务端状态也要能读）。
        if state.is_ready() || cached.object.is_some() {
            let object: TranslationObject = serde_json::from_str(cached.object.as_deref()?).ok()?;
            return Some(TranslationView::from_object(content.blocks.len(), object));
        }
        if state.is_running() || state == TranslationState::Waiting {
            return Some(TranslationView::Pending {
                state,
                due_at: None,
            });
        }
        Some(TranslationView::NotAvailable)
    }

    /// 把一次译文视图写入本地缓存。
    fn cache_translation_view(
        &self,
        content_key: &str,
        lang: &str,
        view: &TranslationView,
    ) -> Result<(), TelaError> {
        match view {
            TranslationView::Ready { object, .. } => {
                let body = serde_json::to_string(object).map_err(|error| TelaError::Config {
                    detail: format!("译本对象无法序列化：{error}"),
                })?;
                self.lock_store()?
                    .put_translation(content_key, lang, &object.status, Some(&body))
            }
            TranslationView::Pending { state, .. } => {
                self.lock_store()?
                    .put_translation(content_key, lang, state.as_str(), None)
            }
            TranslationView::NotAvailable => {
                self.lock_store()?
                    .put_translation(content_key, lang, TRANSLATION_STATE_NONE, None)
            }
        }
    }

    /// 向远端查询一次译文并写缓存（正文已经在本地的场景）。
    fn refresh_translation(
        &self,
        content: &ContentObject,
        lang: &str,
    ) -> Result<TranslationView, TelaError> {
        // 离线时不发译文状态请求；恢复后阅读面板会自动继续。
        self.ensure_online()?;
        let view = self.fetch_translation_view(content, lang)?;
        self.cache_translation_view(&content.key, lang, &view)?;
        Ok(view)
    }

    // ---- 批量离线缓存 ----

    /// 订阅下可缓存的文章：`(articleId, contentKey)`，按列表顺序。
    fn feed_articles(&self, feed_id: i64) -> Result<Vec<(i64, String)>, TelaError> {
        let store = self.lock_store()?;
        let mut articles: Vec<ArticleRow> = store.rows_as("articles")?;
        articles.retain(|article| article.feed_id == feed_id);
        articles.sort_by(|left, right| {
            right
                .sort_at
                .cmp(&left.sort_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(articles
            .into_iter()
            .filter_map(|article| {
                let raw = article.content_key?;
                let key = normalize_content_key(&raw);
                if key.is_empty() {
                    None
                } else {
                    Some((article.id, key))
                }
            })
            .collect())
    }

    /// 缓存该订阅下所有未缓存的原文与译文（阻塞；调用方放到后台执行器）。
    ///
    /// 返回 `Ok` 表示任务跑完（个别文章失败记录在 `report.failed` / `errors` 里）；
    /// 只有本地库读写失败才返回 `Err`。
    pub fn cache_feed_impl(
        &self,
        feed_id: i64,
        progress: &(dyn Fn(OfflineCacheProgress) + Send + Sync),
    ) -> Result<OfflineCacheReport, TelaError> {
        // 点按钮时先判断网络：完全离线就快速失败，而不是逐篇等超时。
        self.probe_network_impl()?;
        let articles = self.feed_articles(feed_id)?;
        let total = articles.len();
        let mut snapshot = OfflineCacheProgress {
            feed_id,
            total,
            ..OfflineCacheProgress::default()
        };
        let mut report = OfflineCacheReport {
            total,
            ..OfflineCacheReport::default()
        };
        progress(snapshot.clone());

        let mut pending: Vec<PendingTranslation> = Vec::new();
        let mut quota_blocked = 0usize;
        let mut error_keys: HashSet<String> = HashSet::new();

        for (index, (article_id, content_key)) in articles.iter().enumerate() {
            let article_id = *article_id;
            let request = ChapterRequest::new(content_key.clone(), self.lang.clone());
            let cached_body = self.cached_body(content_key);
            let cached_view = cached_body
                .as_ref()
                .and_then(|content| self.cached_translation_view(content, &self.lang));
            let cached_ready = matches!(cached_view, Some(TranslationView::Ready { .. }));

            // 正文与译文都完整时不再下载文本，但仍要补齐还没缓存的图片。
            if let (Some(content), Some(view)) = (cached_body.as_ref(), cached_view.as_ref())
                && matches!(view, TranslationView::Ready { .. })
            {
                self.cache_article_images(
                    content,
                    view,
                    article_id,
                    &mut report,
                    &mut snapshot,
                    &mut error_keys,
                )?;
                report.skipped += 1;
                snapshot.skipped += 1;
                snapshot.done = index + 1;
                snapshot.images_failed = report.images_failed;
                progress(snapshot.clone());
                continue;
            }

            // 原文：本地没有才下载；已有正文时只补译文（不重复下载不可变对象）。
            let (content, translation) = match cached_body {
                Some(content) => match self.refresh_translation(&content, &self.lang) {
                    Ok(view) => (content, Some(view)),
                    Err(error) => {
                        record_error(&mut report, &mut error_keys, &error, article_id);
                        report.failed += 1;
                        snapshot.failed += 1;
                        snapshot.done = index + 1;
                        progress(snapshot.clone());
                        continue;
                    }
                },
                None => match self.fetch_chapter(&request) {
                    Ok(chapter) => {
                        self.cache_body(&chapter.content)?;
                        self.cache_translation_view(
                            &chapter.content.key,
                            &self.lang,
                            &chapter.translation,
                        )?;
                        report.originals_cached += 1;
                        snapshot.originals += 1;
                        (chapter.content, Some(chapter.translation))
                    }
                    Err(error) => {
                        record_error(&mut report, &mut error_keys, &error, article_id);
                        report.failed += 1;
                        snapshot.failed += 1;
                        snapshot.done = index + 1;
                        progress(snapshot.clone());
                        continue;
                    }
                },
            };

            // 译文：已就绪就计数；pending 留到轮询；没有则按需发起翻译。
            match translation.as_ref() {
                Some(TranslationView::Ready { .. }) => {
                    if !cached_ready {
                        report.translations_cached += 1;
                        snapshot.translations += 1;
                    }
                }
                Some(TranslationView::Pending { .. }) => {
                    pending.push(PendingTranslation {
                        article_id,
                        content_key: content_key.clone(),
                    });
                }
                // 同语种不需要翻译；其余情况尝试向服务端发起一次。
                Some(TranslationView::NotAvailable)
                    if !same_language(&content.lang, &self.lang) =>
                {
                    match self.request_translation_for(article_id, content_key, &mut report)? {
                        RequestOutcome::Ready => {
                            match self.refresh_translation(&content, &self.lang) {
                                Ok(TranslationView::Ready { .. }) => {
                                    report.translations_cached += 1;
                                    snapshot.translations += 1;
                                }
                                // 服务端说 ready 但取不到译本对象：作者可能关闭了
                                // 翻译，不要把它留在 pending 里空转。
                                Ok(_) => {}
                                Err(error) => {
                                    record_error(&mut report, &mut error_keys, &error, article_id);
                                    pending.push(PendingTranslation {
                                        article_id,
                                        content_key: content_key.clone(),
                                    });
                                }
                            }
                        }
                        RequestOutcome::Pending => pending.push(PendingTranslation {
                            article_id,
                            content_key: content_key.clone(),
                        }),
                        RequestOutcome::Unavailable => {}
                        RequestOutcome::Refused => quota_blocked += 1,
                        RequestOutcome::Failed => {
                            report.failed += 1;
                            snapshot.failed += 1;
                        }
                    }
                }
                // 源语言与目标语言相同：无需翻译。
                Some(TranslationView::NotAvailable) => {}
                None => {}
            }

            // 原文与当前能拿到的译文里的图片都一并缓存（失败/离线留到下次点击）。
            if let Some(view) = translation.as_ref() {
                self.cache_article_images(
                    &content,
                    view,
                    article_id,
                    &mut report,
                    &mut snapshot,
                    &mut error_keys,
                )?;
            }

            snapshot.done = index + 1;
            snapshot.pending = pending.len() + quota_blocked;
            snapshot.images_failed = report.images_failed;
            progress(snapshot.clone());
        }

        // 服务端翻译是异步的：在有限时间内轮询 pending，超过就留给下次点击。
        for _round in 0..PREFETCH_POLL_ROUNDS {
            if pending.is_empty() {
                break;
            }
            std::thread::sleep(PREFETCH_POLL_INTERVAL);
            let mut still = Vec::new();
            for item in pending.drain(..) {
                let Some(content) = self.cached_body(&item.content_key) else {
                    continue;
                };
                match self.refresh_translation(&content, &self.lang) {
                    Ok(view @ TranslationView::Ready { .. }) => {
                        report.translations_cached += 1;
                        snapshot.translations += 1;
                        self.cache_article_images(
                            &content,
                            &view,
                            item.article_id,
                            &mut report,
                            &mut snapshot,
                            &mut error_keys,
                        )?;
                    }
                    Ok(TranslationView::Pending { .. }) => still.push(item),
                    // 作者关闭翻译/服务端放弃：不再排队。
                    Ok(TranslationView::NotAvailable) => {}
                    Err(error) => {
                        record_error(&mut report, &mut error_keys, &error, item.article_id);
                        still.push(item);
                    }
                }
            }
            pending = still;
            snapshot.pending = pending.len() + quota_blocked;
            progress(snapshot.clone());
        }

        report.pending = pending.len() + quota_blocked;
        snapshot.pending = report.pending;
        snapshot.finished = false;
        progress(snapshot);
        Ok(report)
    }

    /// 为没有译文的文章发起一次翻译请求。
    fn request_translation_for(
        &self,
        article_id: i64,
        content_key: &str,
        report: &mut OfflineCacheReport,
    ) -> Result<RequestOutcome, TelaError> {
        let answer = match self.track(self.inner.request_translation(article_id, &self.lang)) {
            Ok(answer) => answer,
            Err(error) => {
                report
                    .errors
                    .push(format!("文章 {article_id} 请求翻译失败：{error}"));
                report.errors.truncate(MAX_REPORT_ERRORS);
                return Ok(RequestOutcome::Failed);
            }
        };
        match answer.status {
            TranslationAnswerStatus::Ready => Ok(RequestOutcome::Ready),
            TranslationAnswerStatus::Refused => {
                report.errors.push(format!(
                    "文章 {article_id}：今日翻译额度已用完，译文明天恢复"
                ));
                report.errors.truncate(MAX_REPORT_ERRORS);
                Ok(RequestOutcome::Refused)
            }
            TranslationAnswerStatus::Unavailable | TranslationAnswerStatus::Unknown => {
                Ok(RequestOutcome::Unavailable)
            }
            TranslationAnswerStatus::Running | TranslationAnswerStatus::Waiting => {
                if let Some(row) = &answer.translation {
                    self.store_translation_row(content_key, &self.lang, row)?;
                }
                Ok(RequestOutcome::Pending)
            }
        }
    }

    fn store_translation_row(
        &self,
        content_key: &str,
        lang: &str,
        row: &TranslationRow,
    ) -> Result<(), TelaError> {
        self.lock_store()?
            .put_translation(content_key, lang, row.state.as_str(), None)
    }
}

impl ContentApi for CachingContentApi {
    fn load_chapter(&self, request: &ChapterRequest) -> Result<Chapter, TelaError> {
        let Some(content) = self.cached_body(&request.content_key) else {
            // 第一次打开：远端取回后立即落库。
            let chapter = self.fetch_chapter(request)?;
            self.cache_body(&chapter.content)?;
            self.cache_translation_view(&chapter.content.key, &request.lang, &chapter.translation)?;
            return Ok(chapter);
        };

        // 正文已经缓存：译文快照（ready/pending/none）也直接本地读，不再联网。
        // 服务端后续翻译完成由阅读面板的 `refresh_chapter` 轮询补齐。
        if let Some(view) = self.cached_translation_view(&content, &request.lang) {
            return Ok(Chapter::new(content, view));
        }

        // 只有“正文有缓存、译文从未取过”才补一次远端查询。
        match self.refresh_translation(&content, &request.lang) {
            Ok(view) => Ok(Chapter::new(content, view)),
            // 离线/令牌过期时退回原文与最近一次快照，正文永远可读。
            Err(error) if error.needs_login() || matches!(error, TelaError::Net { .. }) => {
                let view = self
                    .cached_translation_view(&content, &request.lang)
                    .unwrap_or(TranslationView::NotAvailable);
                Ok(Chapter::new(content, view))
            }
            Err(error) => Err(error),
        }
    }

    fn refresh_chapter(&self, request: &ChapterRequest) -> Result<Chapter, TelaError> {
        // 轮询译文时绝不重新下载不可变正文。
        let Some(content) = self.cached_body(&request.content_key) else {
            let chapter = self.fetch_chapter(request)?;
            self.cache_body(&chapter.content)?;
            self.cache_translation_view(&chapter.content.key, &request.lang, &chapter.translation)?;
            return Ok(chapter);
        };
        // 已经就绪的译文不需要再查一次（例如手动刷新时）。
        if let Some(view) = self.cached_translation_view(&content, &request.lang)
            && matches!(view, TranslationView::Ready { .. })
        {
            return Ok(Chapter::new(content, view));
        }
        let view = self.refresh_translation(&content, &request.lang)?;
        Ok(Chapter::new(content, view))
    }

    fn is_offline(&self) -> bool {
        self.offline.load(Ordering::SeqCst)
    }

    fn probe_network(&self) -> Result<(), TelaError> {
        self.probe_network_impl()
    }

    fn request_translation(
        &self,
        article_id: i64,
        lang: &str,
    ) -> Result<TranslationAnswer, TelaError> {
        self.ensure_online()?;
        self.track(self.inner.request_translation(article_id, lang))
    }

    fn translation_status(
        &self,
        content_key: &str,
        lang: &str,
    ) -> Result<Option<TranslationRow>, TelaError> {
        self.ensure_online()?;
        self.track(self.inner.translation_status(content_key, lang))
    }

    fn load_translation_object(&self, path: &str) -> Result<TranslationObject, TelaError> {
        self.ensure_online()?;
        self.track(self.inner.load_translation_object(path))
    }

    fn load_image(&self, src: &str) -> Result<Vec<u8>, TelaError> {
        // 图片第一次下载后落库；之后（包括离线）直接读本地。
        if let Some(body) = self.cached_image(src) {
            return Ok(body);
        }
        self.fetch_image(src)
    }
}

impl OfflineCacheService for CachingContentApi {
    fn cache_feed(
        &self,
        feed_id: i64,
        progress: &(dyn Fn(OfflineCacheProgress) + Send + Sync),
    ) -> Result<OfflineCacheReport, TelaError> {
        self.cache_feed_impl(feed_id, progress)
    }
}

/// 一条等待服务端翻译完成的文章。
struct PendingTranslation {
    article_id: i64,
    content_key: String,
}

/// 一次「请求翻译」的结果分类。
enum RequestOutcome {
    /// 服务端说已经就绪，可立即再取一次译本对象。
    Ready,
    /// 排队/生成中（或请求暂时失败），下次轮询/点击继续。
    Pending,
    /// 作者关闭翻译或没有正文。
    Unavailable,
    /// 今日额度用完。
    Refused,
    /// 请求本身失败（网络/令牌）；原文仍可用，下次点击重试。
    Failed,
}

/// 失败提示去重后写入报告。
fn record_error(
    report: &mut OfflineCacheReport,
    seen: &mut HashSet<String>,
    error: &TelaError,
    article_id: i64,
) {
    let message = format!("文章 {article_id}：{error}");
    if seen.insert(message.clone()) {
        report.errors.push(message);
        report.errors.truncate(MAX_REPORT_ERRORS);
    }
}

/// 源语言与目标语言是否相同（相同就不必请求翻译）。
fn same_language(source: &str, target: &str) -> bool {
    let source = source.trim().replace('_', "-").to_ascii_lowercase();
    let target = target.trim().replace('_', "-").to_ascii_lowercase();
    if source.is_empty() || target.is_empty() {
        return false;
    }
    if source == target {
        return true;
    }
    // `en` / `en-US` 这类同语种写法等价；中文必须区分简繁。
    if source.starts_with("zh") || target.starts_with("zh") {
        return false;
    }
    source.split('-').next() == target.split('-').next()
}
