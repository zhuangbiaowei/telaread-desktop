//! 阅读面板（S3）：把 [`ReaderSession`] 接到 GPUI，负责异步取数、三视图、翻页与重试。
//!
//! 关键边界：
//! - 正文取数在后台执行器里跑，**不阻塞渲染线程**；
//! - 语义仍由 [`ReaderSession`] 决定（段落对齐、三视图、阅读位置），GUI 只画 `ReaderRow`；
//! - 每次加载带 generation，切文章时旧请求的结果会被丢弃，避免“慢请求覆盖新文章”；
//! - 图片通过 `ContentApi::load_image` 带认证下载，解码后走 `ImageSource::Image`，
//!   不把远程 URL 直接交给 GPUI（否则会绕过认证与缓存）；
//! - 没有译本时自动向服务端 `POST /v1/translations` 请求，并在后台轮询；
//!   译文始终由服务端生成，本地不伪造翻译。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tela_content::api::{ChapterRequest, ContentApi};
use tela_content::model::TranslationAnswerStatus;

use crate::reader::document::{ReaderDocument, TranslationStatus, preformatted_text};
use crate::reader::session::{ReaderSession, ReaderState};
use crate::reader::{ReaderRow, ViewMode};
use crate::shell::ShellData;

use gpui_kit::base::text::{TextView, TextViewStyle};
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const BG: u32 = 0x11111b;
const PANEL: u32 = 0x1e1e2e;
const BORDER: u32 = 0x313244;
const TEXT: u32 = 0xcdd6f4;
const MUTED: u32 = 0x6c7086;
const SUBTLE: u32 = 0xa6adc8;
const ACCENT: u32 = 0x89b4fa;
const DANGER: u32 = 0xf38ba8;
const WARN: u32 = 0xf9e2af;

/// 阅读正文的最大行宽（单栏时居中）。
const READING_MAX_WIDTH: f32 = 760.0;

/// 翻译轮询间隔与最多轮数（约一分钟）。
const TRANSLATION_POLL_INTERVAL: Duration = Duration::from_millis(1600);
const TRANSLATION_POLL_ATTEMPTS: usize = 36;

/// 阅读面板视图。
pub struct ReaderView {
    data: Arc<dyn ShellData>,
    content: Arc<dyn ContentApi>,
    session: ReaderSession,
    article_id: Option<i64>,
    title: Option<String>,
    generation: u64,
    focus: FocusHandle,
    focused: bool,
    task: Option<Task<()>>,
    translation_task: Option<Task<()>>,
    /// 已经请求过的 `(contentKey, lang)`，避免每帧重复 POST。
    requested_translations: HashSet<String>,
    translation_notice: Option<String>,
    /// 已解码的图片（src → 内存图），供 `TextView` 的 image_source 解析。
    image_cache: Arc<Mutex<HashMap<String, Arc<Image>>>>,
    /// 正在下载的图片，避免重复请求。
    image_pending: Arc<Mutex<HashSet<String>>>,
    image_tasks: Vec<Task<()>>,
    /// 正文滚动容器；PageUp/PageDown 按视口高度滚动，而不是按段落分页。
    scroll_handle: ScrollHandle,
}

impl ReaderView {
    /// 新建阅读面板；初始没有文章，显示“选择文章”空态。
    pub fn new(
        data: Arc<dyn ShellData>,
        content: Arc<dyn ContentApi>,
        cx: &mut Context<Self>,
    ) -> Self {
        let session = ReaderSession::new(
            Arc::clone(&content),
            ChapterRequest::new(String::new(), "zh-Hans"),
        );
        Self {
            data,
            content,
            session,
            article_id: None,
            title: None,
            generation: 0,
            focus: cx.focus_handle(),
            focused: false,
            task: None,
            translation_task: None,
            requested_translations: HashSet::new(),
            translation_notice: None,
            image_cache: Arc::new(Mutex::new(HashMap::new())),
            image_pending: Arc::new(Mutex::new(HashSet::new())),
            image_tasks: Vec::new(),
            scroll_handle: ScrollHandle::new(),
        }
    }

    /// 当前阅读会话（测试与状态栏用）。
    pub fn session(&self) -> &ReaderSession {
        &self.session
    }

    /// 当前文章 id。
    pub fn article_id(&self) -> Option<i64> {
        self.article_id
    }

    /// 当前翻译提示（请求中 / 失败 / 不提供）。
    pub fn translation_notice(&self) -> Option<&str> {
        self.translation_notice.as_deref()
    }

    /// 正文滚动偏移（测试/诊断用；向下滚动时 y 为负）。
    pub fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll_handle.offset()
    }

    /// 选中文章：换正文请求、进入加载态，并在后台取数。
    ///
    /// 重复选中同一篇不做任何事（避免每次点击都重新请求）；
    /// 失败后重试走 [`Self::reload`]。
    pub fn select(&mut self, article_id: i64, title: String, cx: &mut Context<Self>) {
        // 同一篇且当前不是失败态：不重复请求。失败态下再点一次等于重试。
        if self.article_id == Some(article_id) && !self.session.state().is_failed() {
            return;
        }
        self.article_id = Some(article_id);
        self.title = Some(title);
        self.translation_notice = None;
        self.translation_task = None;
        self.scroll_handle.set_offset(Point {
            x: px(0.),
            y: px(0.),
        });
        match self.data.chapter_request(article_id) {
            Ok(request) => {
                self.session.reset_to(request);
                self.spawn_load(cx);
            }
            Err(error) => {
                // 本地没有正文对象：直接进入失败态，错误页里的“重试”仍有意义。
                self.session.apply_load_result(Err(error));
                cx.notify();
            }
        }
    }

    /// 重试当前文章（后台取数）。
    ///
    /// 如果上次连 `chapter_request` 都失败了（request 为空），重试会先重新解析
    /// 文章 → 正文 key；这样“服务端还没就绪”之类的暂时性错误恢复后无需重新选文章。
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(article_id) = self.article_id else {
            return;
        };
        if self.session.request().content_key.is_empty() {
            match self.data.chapter_request(article_id) {
                Ok(request) => self.session.reset_to(request),
                Err(error) => {
                    self.session.mark_retry();
                    self.session.apply_load_result(Err(error));
                    cx.notify();
                    return;
                }
            }
        }
        // 手动重试允许重新请求一次翻译。
        self.forget_translation_request();
        self.translation_notice = None;
        self.translation_task = None;
        self.session.mark_retry();
        self.session.begin_load();
        self.spawn_load(cx);
        cx.notify();
    }

    /// 切换视图；返回是否变化。
    pub fn set_mode(&mut self, mode: ViewMode) -> bool {
        if self.session.mode() == mode {
            return false;
        }
        self.session.set_mode(mode);
        true
    }

    /// 段落分页的下一页；返回是否变化。新页面从顶部开始。
    pub fn next_page(&mut self) -> bool {
        let changed = self.session.next_page();
        if changed {
            self.scroll_handle.set_offset(Point {
                x: px(0.),
                y: px(0.),
            });
        }
        changed
    }

    /// 段落分页的上一页；返回是否变化。新页面停在底部，接着往上读。
    pub fn prev_page(&mut self) -> bool {
        let changed = self.session.prev_page();
        if changed {
            self.scroll_handle.scroll_to_bottom();
        }
        changed
    }

    /// PageDown：先按视口高度向下滚；到底后再切到下一段落页。
    pub fn page_down(&mut self) -> bool {
        let offset = self.scroll_handle.offset();
        let max = self.scroll_handle.max_offset().y;
        if max > px(1.) && offset.y > -max + px(1.) {
            let step = page_step(self.scroll_handle.bounds().size.height);
            let target = (offset.y - step).max(-max);
            self.scroll_handle.set_offset(Point {
                x: offset.x,
                y: target,
            });
            return true;
        }
        self.next_page()
    }

    /// PageUp：先按视口高度向上滚；到顶后再切到上一段落页的底部。
    pub fn page_up(&mut self) -> bool {
        let offset = self.scroll_handle.offset();
        if offset.y < px(-1.) {
            let step = page_step(self.scroll_handle.bounds().size.height);
            let target = (offset.y + step).min(px(0.));
            self.scroll_handle.set_offset(Point {
                x: offset.x,
                y: target,
            });
            return true;
        }
        self.prev_page()
    }

    /// 键盘快捷键：`o` 原文 / `c` 对照 / `t` 译文 / `n` 段落下一页 / `p` 段落上一页 /
    /// `PageUp`/`PageDown` 视口滚动（到头再切段落页）/ `r` 重试。
    ///
    /// 返回是否发生变化；调用方负责 `cx.notify()`。
    pub fn handle_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        match key {
            "o" => self.set_mode(ViewMode::Original),
            "c" => self.set_mode(ViewMode::Compare),
            "t" => self.set_mode(ViewMode::Translation),
            "n" => self.next_page(),
            "p" => self.prev_page(),
            "pagedown" => self.page_down(),
            "pageup" => self.page_up(),
            "r" => {
                self.reload(cx);
                true
            }
            _ => false,
        }
    }

    fn spawn_load(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let request = self.session.request().clone();
        let api = Arc::clone(&self.content);
        let task = cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = cx
                .background_executor()
                .spawn(async move { api.load_chapter(&request) })
                .await;
            let _ = this.update(cx, |reader, cx| {
                // 文章的快速切换会让旧请求晚到；generation 不匹配就丢弃。
                if reader.generation == generation {
                    reader.session.apply_load_result(result);
                    reader.after_load(cx);
                    cx.notify();
                }
            });
        });
        self.task = Some(task);
    }

    /// 正文就绪后的副作用：下载图片、必要时请求/轮询译文。
    fn after_load(&mut self, cx: &mut Context<Self>) {
        self.start_image_loads(cx);
        self.maybe_translate(cx);
    }

    /// 把当前译文请求从“已请求”集合里移除（手动重试用）。
    fn forget_translation_request(&mut self) {
        if let Some(doc) = self.session.state().document() {
            let key = translation_key(&doc.content_key, &doc.lang);
            self.requested_translations.remove(&key);
        }
    }

    /// 没有译文时自动请求服务端翻译；正在翻译时只轮询，不重复 POST。
    fn maybe_translate(&mut self, cx: &mut Context<Self>) {
        let Some(article_id) = self.article_id else {
            return;
        };
        let Some(doc) = self.session.state().document() else {
            return;
        };
        let status = doc.translation_status;
        if matches!(
            status,
            TranslationStatus::Complete | TranslationStatus::Partial
        ) {
            return;
        }
        if same_language(&doc.source_lang, &doc.lang) {
            return;
        }

        let key = translation_key(&doc.content_key, &doc.lang);
        if !self.requested_translations.insert(key.clone()) {
            return;
        }

        let request = self.session.request().clone();
        let send_request = status == TranslationStatus::Missing;
        self.translation_notice = Some(if send_request {
            "正在向服务端请求译文，完成前先显示原文…".to_string()
        } else {
            "服务端正在翻译，先显示原文…".to_string()
        });
        self.spawn_translation(article_id, request, key, send_request, cx);
        cx.notify();
    }

    /// 后台请求翻译并按需轮询；切文章后旧任务的结果会被 generation 丢弃。
    fn spawn_translation(
        &mut self,
        article_id: i64,
        request: ChapterRequest,
        key: String,
        send_request: bool,
        cx: &mut Context<Self>,
    ) {
        let generation = self.generation;
        let api = Arc::clone(&self.content);
        let task = cx.spawn(async move |this, cx: &mut AsyncApp| {
            let mut should_poll = true;

            if send_request {
                let request_api = Arc::clone(&api);
                let lang = request.lang.clone();
                let answer = cx
                    .background_executor()
                    .spawn(async move { request_api.request_translation(article_id, &lang) })
                    .await;
                match answer {
                    Ok(answer) => match answer.status {
                        // 服务端说已经就绪：立即取一次译本对象。
                        TranslationAnswerStatus::Ready => should_poll = false,
                        TranslationAnswerStatus::Running | TranslationAnswerStatus::Waiting => {}
                        TranslationAnswerStatus::Refused => {
                            let notice = "今日翻译额度已用完，译文明天恢复；现在可以读原文。";
                            set_translation_notice(&this, cx, generation, article_id, notice);
                            return;
                        }
                        TranslationAnswerStatus::Unavailable | TranslationAnswerStatus::Unknown => {
                            let notice = "服务端没有提供这篇的翻译（作者关闭翻译或正文为空）。";
                            set_translation_notice(&this, cx, generation, article_id, notice);
                            return;
                        }
                    },
                    Err(error) => {
                        let notice = format!("翻译请求失败：{error}");
                        set_translation_notice(&this, cx, generation, article_id, &notice);
                        return;
                    }
                }
            }

            let attempts = if should_poll {
                TRANSLATION_POLL_ATTEMPTS
            } else {
                1
            };
            for attempt in 0..attempts {
                let delay = if attempt == 0 {
                    Duration::from_millis(600)
                } else {
                    TRANSLATION_POLL_INTERVAL
                };
                cx.background_executor().timer(delay).await;

                let request_api = Arc::clone(&api);
                let next_request = request.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { request_api.load_chapter(&next_request) })
                    .await;
                let Ok(chapter) = result else {
                    continue;
                };

                let document = ReaderDocument::from_chapter(&request, &chapter);
                let status = document.translation_status;
                let done = matches!(
                    status,
                    TranslationStatus::Complete | TranslationStatus::Partial
                );
                let missing = matches!(status, TranslationStatus::Missing);
                let key = key.clone();
                let update = this.update(cx, |reader, cx| {
                    if reader.generation != generation || reader.article_id != Some(article_id) {
                        return;
                    }
                    reader.session.apply_load_result(Ok(chapter));
                    reader.translation_notice = Some(translation_notice_for(status));
                    if done {
                        reader.requested_translations.remove(&key);
                    }
                    reader.start_image_loads(cx);
                    cx.notify();
                });
                if update.is_err() {
                    return;
                }
                if done {
                    return;
                }
                // `Ready` 但加载后仍没有译本对象：不再空转轮询，交给用户按 r 重试。
                if !should_poll && missing {
                    let notice = "译文请求已提交，服务端生成后可重新打开或按 r 重试。";
                    set_translation_notice(&this, cx, generation, article_id, notice);
                    return;
                }
            }

            if should_poll {
                let notice = "译文仍在生成中，可稍后按 r 重试。";
                set_translation_notice(&this, cx, generation, article_id, notice);
            }
        });
        self.translation_task = Some(task);
    }

    /// 收集当前文档里的图片，逐个在后台带认证下载。
    fn start_image_loads(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.session.state().document() else {
            return;
        };
        let sources: Vec<String> = doc.image_sources.clone();
        if sources.is_empty() {
            return;
        }

        for src in sources {
            let cached = self
                .image_cache
                .lock()
                .map(|cache| cache.contains_key(&src))
                .unwrap_or(false);
            if cached {
                continue;
            }
            let already_loading = self
                .image_pending
                .lock()
                .map(|mut pending| !pending.insert(src.clone()))
                .unwrap_or(false);
            if already_loading {
                continue;
            }

            let api = Arc::clone(&self.content);
            let cache = Arc::clone(&self.image_cache);
            let pending = Arc::clone(&self.image_pending);
            let load_src = src.clone();
            let task = cx.spawn(async move |this, cx: &mut AsyncApp| {
                let result = cx
                    .background_executor()
                    .spawn(async move { api.load_image(&load_src) })
                    .await;
                if let Ok(bytes) = result
                    && let Some(image) = decode_image(&bytes)
                    && let Ok(mut cache) = cache.lock()
                {
                    cache.insert(src.clone(), image);
                }
                if let Ok(mut pending) = pending.lock() {
                    pending.remove(&src);
                }
                let _ = this.update(cx, |_, cx| cx.notify());
            });
            self.image_tasks.push(task);
        }

        // 任务句柄只为保活；长期阅读不需要无限增长。
        if self.image_tasks.len() > 128 {
            self.image_tasks.drain(0..64);
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        if self.article_id.is_none() {
            return v_flex()
                .w_full()
                .gap_1()
                .p_4()
                .bg(rgb(PANEL))
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("阅读"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("从中间列表选择一篇文章开始阅读"),
                );
        }

        let title = self.title.clone().unwrap_or_else(|| "阅读".to_string());
        let status = self.session.state().status_label();
        let page = self.session.page() + 1;
        let pages = self.session.page_count();
        let page_suffix = if pages > 1 {
            format!(" · 第 {page}/{pages} 页")
        } else {
            String::new()
        };
        let mode = self.session.mode();

        let mut modes = Vec::new();
        for option in ViewMode::ALL {
            let active = mode == option;
            modes.push(
                div()
                    .id(format!("reader-mode-{}", option.as_str()))
                    .role(Role::Button)
                    .aria_label(option.label())
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(active, |element| {
                        element.bg(rgb(ACCENT)).text_color(rgb(BG))
                    })
                    .when(!active, |element| {
                        element.hover(|style| style.bg(rgb(BORDER)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.set_mode(option) {
                            cx.notify();
                        }
                    }))
                    .child(option.label()),
            );
        }

        let can_prev = self.session.page() > 0;
        let can_next = self.session.page() + 1 < pages;
        let notice = self.translation_notice.clone();

        v_flex()
            .w_full()
            .gap_2()
            .p_4()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                h_flex()
                    .items_start()
                    .justify_between()
                    .gap_4()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(SUBTLE))
                                    .child(format!("{status}{page_suffix}")),
                            )
                            .when_some(notice, |element, notice| {
                                element.child(div().text_xs().text_color(rgb(WARN)).child(notice))
                            }),
                    )
                    .child(h_flex().items_center().gap_1().children(modes)),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("共 {} 段", self.session.total_paragraphs())),
                    )
                    // 只有一段分页时不要显示“上一页 1/1 下一页”：正文超出
                    // 一屏也没关系，PageUp/PageDown 会按视口滚动。
                    .when(pages > 1, |row| {
                        row.child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(Self::nav_button("上一页", can_prev, cx, true))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(SUBTLE))
                                        .child(format!("{page}/{pages}")),
                                )
                                .child(Self::nav_button("下一页", can_next, cx, false)),
                        )
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("o/c/t 视图 · j/k 文章 · Shift+j/k 博客 · PgUp/PgDn 翻页"),
            )
    }

    fn nav_button(
        label: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
        prev: bool,
    ) -> impl IntoElement {
        div()
            .id(if prev {
                "reader-page-prev"
            } else {
                "reader-page-next"
            })
            .role(Role::Button)
            .aria_label(label)
            .px_2()
            .py_1()
            .rounded_md()
            .when(enabled, |element| {
                element
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(BORDER)))
            })
            .when(!enabled, |element| element.opacity(0.4))
            .on_click(cx.listener(move |this, _, _, cx| {
                let changed = if prev {
                    this.prev_page()
                } else {
                    this.next_page()
                };
                if changed {
                    cx.notify();
                }
            }))
            .child(label)
    }

    fn body(&self, cx: &mut Context<Self>) -> Div {
        if self.article_id.is_none() {
            return self.centered_note("选择一篇文章开始阅读", "左侧选订阅，中间选文章。");
        }
        match self.session.state() {
            ReaderState::Loading => self.centered_note("加载中…", "正在取回正文与译文。"),
            ReaderState::Empty => self.centered_note(
                "本章没有正文",
                "服务端返回了空章节，这不是错误；可以换一篇文章。",
            ),
            ReaderState::Failed { message, retries } => {
                let detail = format!("{message}（已重试 {retries} 次）");
                self.error_body(&detail, cx)
            }
            ReaderState::Ready(_) => self.ready_body(),
        }
    }

    fn ready_body(&self) -> Div {
        let mode = self.session.mode();
        let rows = self.session.visible_rows();
        let mut list = v_flex()
            .id("reader-scroll")
            .flex_1()
            .min_h_0()
            .items_center()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .px_6()
            .py_4();

        if rows.is_empty() {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("这一页没有可显示的段落。"),
            );
        } else {
            list = list.child(self.column_header(mode));
            for row in &rows {
                list = list.child(self.row_block(row, mode));
            }
        }
        v_flex().flex_1().min_h_0().child(list)
    }

    /// 顶部只出现一次的列标题（原文 / 译文）。
    fn column_header(&self, mode: ViewMode) -> Div {
        let Some(doc) = self.session.state().document() else {
            return div().w_full();
        };
        let original = format!("原文 · {}", language_label(&doc.source_lang));
        let translated = format!("译文 · {}", language_label(&doc.lang));

        let label = |text: String| div().text_xs().text_color(rgb(MUTED)).child(text);

        match mode {
            ViewMode::Compare => h_flex()
                .w_full()
                .items_center()
                .pb_2()
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(div().flex_1().min_w_0().pr_6().child(label(original)))
                .child(div().w(px(1.)).h(px(12.)).bg(rgb(BORDER)))
                .child(div().flex_1().min_w_0().pl_6().child(label(translated))),
            ViewMode::Original => v_flex()
                .w_full()
                .max_w(px(READING_MAX_WIDTH))
                .pb_2()
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(label(original)),
            ViewMode::Translation => v_flex()
                .w_full()
                .max_w(px(READING_MAX_WIDTH))
                .pb_2()
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(label(translated)),
        }
    }

    fn row_block(&self, row: &ReaderRow, mode: ViewMode) -> Div {
        let original = self.block_view(
            format!("reader-original-{}", row.index),
            row.original_html.clone(),
            &row.tag,
        );

        // 缺译文时回退原文（代码/图片/超额段落仍然可读）；顶部状态条会说明
        // 整篇存在未翻译段落，不在每段中间插占位句。
        let translation_html = row
            .translation_html
            .clone()
            .unwrap_or_else(|| row.original_html.clone());
        let translation = self.block_view(
            format!("reader-translation-{}", row.index),
            translation_html,
            &row.tag,
        );

        match mode {
            ViewMode::Original => v_flex()
                .w_full()
                .max_w(px(READING_MAX_WIDTH))
                .py_3()
                .child(original),
            ViewMode::Translation => v_flex()
                .w_full()
                .max_w(px(READING_MAX_WIDTH))
                .py_3()
                .child(translation),
            // 对照：块级对齐 + 原文/译文之间一道竖线；不再给每段加标签或横线。
            // 纵向留白放在两列内部，竖线才能跨行连续，不被段间距切断。
            ViewMode::Compare => h_flex()
                .w_full()
                .items_stretch()
                .child(v_flex().flex_1().min_w_0().py_3().pr_6().child(original))
                .child(div().w(px(1.)).bg(rgb(BORDER)))
                .child(v_flex().flex_1().min_w_0().py_3().pl_6().child(translation)),
        }
    }

    /// 一个块 → 富文本元素；代码块单独渲染，其余按 HTML（标题/引用/链接/表格/图片）。
    fn block_view(&self, id: String, html: String, tag: &str) -> AnyElement {
        let is_code = tag == "pre" || html.trim_start().to_ascii_lowercase().starts_with("<pre");
        if is_code {
            return self.code_block(id, preformatted_text(&html));
        }
        self.rich_text(id, html).into_any_element()
    }

    /// 等宽代码块；避免 GPUI 把 `<pre><code>` 当成行内代码画出浅色圆角条。
    fn code_block(&self, id: String, code: String) -> AnyElement {
        v_flex()
            .id(id)
            .w_full()
            .min_w_0()
            .rounded_md()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .p_3()
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .font_family("monospace")
                    .text_sm()
                    .line_height(relative(1.6))
                    .text_color(rgb(TEXT))
                    .child(code)
                    .overflow_x_scrollbar(),
            )
            .into_any_element()
    }

    /// 一个块 → 富文本元素；标题、引用、链接、表格和图片按原 HTML 渲染。
    fn rich_text(&self, id: String, html: String) -> impl IntoElement {
        let cache = Arc::clone(&self.image_cache);
        TextView::html(ElementId::Name(id.into()), html)
            .style(text_style())
            .selectable(true)
            .w_full()
            .image_source(move |uri| {
                let src = uri.as_ref();
                // 内联 data: 图由 GPUI 自己处理。
                if src.starts_with("data:") {
                    return ImageSource::from(src.to_string());
                }
                match cache.lock().ok().and_then(|cache| cache.get(src).cloned()) {
                    Some(image) => ImageSource::Image(image),
                    // 还没下载完：先不占位，下载完成后 notify 重绘。
                    None => ImageSource::Custom(Arc::new(
                        |_: &mut Window, _: &mut App| -> Option<
                            Result<Arc<RenderImage>, ImageCacheError>,
                        > { None },
                    )),
                }
            })
    }

    fn centered_note(&self, title: &str, detail: &str) -> Div {
        v_flex()
            .flex_1()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_8()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(detail.to_string()),
            )
    }

    fn error_body(&self, detail: &str, cx: &mut Context<Self>) -> Div {
        v_flex()
            .flex_1()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .p_8()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(DANGER))
                    .child("正文加载失败"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(SUBTLE))
                    .child(detail.to_string()),
            )
            .child(
                div()
                    .id("reader-retry")
                    .role(Role::Button)
                    .aria_label("重试")
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(ACCENT))
                    .text_color(rgb(BG))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.reload(cx);
                    }))
                    .child("重试"),
            )
    }
}

impl Render for ReaderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focused {
            window.focus(&self.focus, cx);
            self.focused = true;
        }
        let header = self.header(cx);
        let body = self.body(cx);
        v_flex()
            .id("reader-root")
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .track_focus(&self.focus)
            .focusable()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.handle_key(&event.keystroke.key, cx) {
                    cx.notify();
                }
            }))
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus, cx);
            }))
            .child(header)
            .child(body)
    }
}

/// 一次 PageUp/PageDown 的滚动距离：视口高度减去少量重叠，避免行被切断。
fn page_step(viewport: Pixels) -> Pixels {
    if viewport > px(96.) {
        viewport - px(48.)
    } else {
        px(80.)
    }
}

/// 正文富文本样式：沿用 Catppuccin 配色，避免 TextView 落到浅色主题。
fn text_style() -> TextViewStyle {
    let inline_code = HighlightStyle {
        background_color: Some(rgb(BORDER).into()),
        color: Some(rgb(TEXT).into()),
        ..Default::default()
    };
    TextViewStyle::default()
        .with_foreground(rgb(TEXT).into())
        .with_muted_foreground(rgb(MUTED).into())
        .with_link(rgb(ACCENT).into())
        .with_selection(rgba(0x89b4fa55).into())
        .with_code_background(rgb(PANEL).into())
        .with_border(rgb(BORDER).into())
        .with_inline_code(inline_code)
        .with_paragraph_gap(rems(0.55))
        .with_dark(true)
}

/// `(contentKey, lang)` → 去重键。
fn translation_key(content_key: &str, lang: &str) -> String {
    format!("{content_key}\u{1f}{lang}")
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

/// 译文状态 → 顶部提示。
fn translation_notice_for(status: TranslationStatus) -> String {
    match status {
        TranslationStatus::Complete => "译文已就绪。".to_string(),
        TranslationStatus::Partial => "译文部分缺失；未翻译段落显示原文。".to_string(),
        TranslationStatus::Pending => "服务端正在翻译，先显示原文…".to_string(),
        TranslationStatus::Missing => "本章暂无译文，已显示原文。".to_string(),
    }
}

/// 在后台任务里设置提示（只有当前文章仍匹配时才写）。
fn set_translation_notice(
    this: &WeakEntity<ReaderView>,
    cx: &mut AsyncApp,
    generation: u64,
    article_id: i64,
    notice: &str,
) {
    let notice = notice.to_string();
    let _ = this.update(cx, |reader, cx| {
        if reader.generation == generation && reader.article_id == Some(article_id) {
            reader.translation_notice = Some(notice);
            cx.notify();
        }
    });
}

/// 按文件头识别图片格式（服务端图片是 `/img/…`，没有扩展名可用）。
fn decode_image(bytes: &[u8]) -> Option<Arc<Image>> {
    if bytes.is_empty() {
        return None;
    }
    let format = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ImageFormat::Png
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        ImageFormat::Jpeg
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        ImageFormat::Gif
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        ImageFormat::Webp
    } else if bytes.starts_with(b"BM") {
        ImageFormat::Bmp
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        ImageFormat::Tiff
    } else if bytes.starts_with(b"\0\0\x01\0") {
        ImageFormat::Ico
    } else if looks_like_svg(bytes) {
        ImageFormat::Svg
    } else {
        return None;
    };
    Some(Arc::new(Image::from_bytes(format, bytes.to_vec())))
}

/// SVG 可能是 `<svg`，也可能先出现 XML 声明或注释。
fn looks_like_svg(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(0);
    let rest = &bytes[start..];
    rest.starts_with(b"<svg")
        || rest.starts_with(b"<?xml")
        || rest.starts_with(b"<!--")
        || rest.starts_with(b"<!DOCTYPE svg")
}

/// 语言码 → 人类可读名称。
fn language_label(code: &str) -> &str {
    match code {
        "zh-Hans" | "zh-hans" | "zh-CN" | "zh-cn" => "简体中文",
        "zh-Hant" | "zh-hant" | "zh-TW" | "zh-tw" | "zh-HK" | "zh-hk" => "繁體中文",
        "en" | "en-US" | "en-us" => "English",
        "fr" | "fr-FR" | "fr-fr" => "Français",
        "ja" | "ja-JP" | "ja-jp" => "日本語",
        other => other,
    }
}
