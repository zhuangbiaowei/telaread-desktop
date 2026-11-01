//! 阅读面板（S3）：把 [`ReaderSession`] 接到 GPUI，负责异步取数、三视图、连续滚动与重试。
//!
//! 关键边界：
//! - 正文取数在后台执行器里跑，**不阻塞渲染线程**；
//! - 语义仍由 [`ReaderSession`] 决定（段落对齐、三视图、阅读位置），GUI 只画渲染缓存里的行；
//! - 正文一次渲染整篇、不做段落分页：PageUp/PageDown 与 ↑/↓ 只滚动视口，
//!   用户一路按下去就能读完，顶部不显示页码；
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

use crate::reader::ViewMode;
use crate::reader::document::{ReaderDocument, TranslationStatus, preformatted_text};
use crate::reader::session::{ReaderSession, ReaderState};
use crate::shell::{RecommendationItem, RecommendationService, ShellData};

use gpui_kit::base::input::{InputEditorStyle, TextareaState};
use gpui_kit::base::text::{TextView, TextViewStyle};
use gpui_kit::base::{
    Dialog, DialogBackdrop, DialogDescription, DialogPopup, DialogTitle, InputBase, Textarea,
    h_flex, v_flex,
};
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
/// 离线时的探活间隔：完全离线时不发译文状态请求，只隔一段时间确认网络是否恢复。
const OFFLINE_PROBE_INTERVAL: Duration = Duration::from_secs(10);

/// 阅读面板向上层发出的通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderEvent {
    /// 当前文章的推荐状态已在本地改变（推荐、改推荐语或取消）。
    RecommendationChanged {
        /// 变化的文章 id。
        article_id: i64,
    },
}

/// 推荐弹窗的临时状态：说明文字输入框与打开时是否已推荐。
struct RecommendDialog {
    /// 推荐语输入框；打开时创建，关闭时释放。
    note: Entity<TextareaState>,
    /// 打开弹窗时这篇文章是否已推荐（决定标题与按钮）。
    editing: bool,
}

/// 一段正文的渲染缓存。
///
/// 把原文/译文的 HTML 预先转成 `SharedString` 缓存下来：滚动时不用每帧
/// 从段落数据重新分配整篇正文的字符串，`TextView` 也能复用同一个分配，
/// 快速判断“文本没变”而跳过重复设置。
struct RenderedRow {
    /// 块索引（作为稳定的元素 id 后缀）。
    index: usize,
    /// 语义标签（代码块判断用）。
    tag: String,
    /// 原文 HTML。
    original_html: SharedString,
    /// 译文 HTML；缺译文时复用原文（与阅读规则一致）。
    translation_html: SharedString,
}

/// 阅读面板视图。
pub struct ReaderView {
    data: Arc<dyn ShellData>,
    content: Arc<dyn ContentApi>,
    session: ReaderSession,
    article_id: Option<i64>,
    /// 当前阅读的本地网页收藏 id（与 `article_id` 互斥）。
    bookmark_id: Option<i64>,
    /// 打开收藏之前的视图模式：收藏固定用原文视图，回到文章时恢复用户原来的选择。
    mode_before_bookmark: Option<ViewMode>,
    title: Option<String>,
    /// 原文地址；用于标题旁的“打开原文”，为空时不显示入口。
    article_url: Option<String>,
    generation: u64,
    focus: FocusHandle,
    focused: bool,
    task: Option<Task<()>>,
    translation_task: Option<Task<()>>,
    /// 已经请求过的 `(contentKey, lang)`，避免每帧重复 POST。
    requested_translations: HashSet<String>,
    translation_notice: Option<String>,
    /// 打开原文失败等与链接有关的提示。
    link_notice: Option<String>,
    /// 已解码的图片（src → 内存图），供 `TextView` 的 image_source 解析。
    image_cache: Arc<Mutex<HashMap<String, Arc<Image>>>>,
    /// 正在下载的图片，避免重复请求。
    image_pending: Arc<Mutex<HashSet<String>>>,
    image_tasks: Vec<Task<()>>,
    /// 正文滚动容器；PageUp/PageDown 按视口高度滚动，Up/Down 按行滚动。
    scroll_handle: ScrollHandle,
    /// 当前窗口正文的行高，渲染时更新；Up/Down 每次滚一行。
    line_height: Pixels,
    /// 当前文章 + 视图下的渲染缓存；文章/视图/译文变化时重建。
    rendered_rows: Vec<RenderedRow>,
    /// 推荐写能力；为空时不显示推荐按钮。
    recommendations: Option<Arc<dyn RecommendationService>>,
    /// 当前文章的推荐（未推荐时为 `None`）。
    recommendation: Option<RecommendationItem>,
    /// 推荐弹窗；`None` 表示关闭。
    recommend_dialog: Option<RecommendDialog>,
    /// 推荐弹窗里的错误（保留弹窗，方便修改后重试）。
    recommend_error: Option<String>,
    /// 推荐操作的一句提示（阅读面板顶部）。
    recommend_notice: Option<String>,
}

impl ReaderView {
    /// 新建阅读面板；初始没有文章，显示“选择文章”空态。
    pub fn new(
        data: Arc<dyn ShellData>,
        content: Arc<dyn ContentApi>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_recommendations(data, content, None, cx)
    }

    /// 带推荐写能力的新建；`recommendations` 为空时推荐按钮不显示。
    pub fn new_with_recommendations(
        data: Arc<dyn ShellData>,
        content: Arc<dyn ContentApi>,
        recommendations: Option<Arc<dyn RecommendationService>>,
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
            bookmark_id: None,
            mode_before_bookmark: None,
            title: None,
            article_url: None,
            generation: 0,
            focus: cx.focus_handle(),
            focused: false,
            task: None,
            translation_task: None,
            requested_translations: HashSet::new(),
            translation_notice: None,
            link_notice: None,
            image_cache: Arc::new(Mutex::new(HashMap::new())),
            image_pending: Arc::new(Mutex::new(HashSet::new())),
            image_tasks: Vec::new(),
            scroll_handle: ScrollHandle::new(),
            line_height: px(24.),
            rendered_rows: Vec::new(),
            recommendations,
            recommendation: None,
            recommend_dialog: None,
            recommend_error: None,
            recommend_notice: None,
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

    /// 当前阅读的收藏 id（读本地网页时非空）。
    pub fn bookmark_id(&self) -> Option<i64> {
        self.bookmark_id
    }

    /// 当前是否有正文可选择（文章与本地收藏共用阅读面板）。
    fn has_selection(&self) -> bool {
        self.article_id.is_some() || self.bookmark_id.is_some()
    }

    /// 当前文章的原文地址（测试/诊断用）。
    pub fn original_url(&self) -> Option<&str> {
        self.article_url.as_deref()
    }

    /// 当前文章的推荐；未推荐时为 `None`。
    pub fn recommendation(&self) -> Option<&RecommendationItem> {
        self.recommendation.as_ref()
    }

    /// 推荐弹窗是否打开（测试/诊断用）。
    pub fn recommend_dialog_open(&self) -> bool {
        self.recommend_dialog.is_some()
    }

    /// 推荐弹窗里的错误提示（测试/诊断用）。
    pub fn recommend_error(&self) -> Option<&str> {
        self.recommend_error.as_deref()
    }

    /// 推荐操作的一句提示（测试/诊断用）。
    pub fn recommend_notice(&self) -> Option<&str> {
        self.recommend_notice.as_deref()
    }

    /// 当前翻译提示（请求中 / 失败 / 不提供）。
    pub fn translation_notice(&self) -> Option<&str> {
        self.translation_notice.as_deref()
    }

    /// 打开原文失败等链接提示。
    pub fn link_notice(&self) -> Option<&str> {
        self.link_notice.as_deref()
    }

    /// 正文滚动偏移（测试/诊断用；向下滚动时 y 为负）。
    pub fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll_handle.offset()
    }

    /// 当前行高（Up/Down 的滚动步长，测试/诊断用）。
    pub fn line_height(&self) -> Pixels {
        self.line_height
    }

    /// 选中文章：换正文请求、进入加载态，并在后台取数。
    ///
    /// 重复选中同一篇不做任何事（避免每次点击都重新请求）；
    /// 失败后重试走 [`Self::reload`]。
    pub fn select(
        &mut self,
        article_id: i64,
        title: String,
        url: Option<String>,
        cx: &mut Context<Self>,
    ) {
        // 同一篇且当前不是失败态：不重复请求。失败态下再点一次等于重试。
        if self.article_id == Some(article_id)
            && self.bookmark_id.is_none()
            && !self.session.state().is_failed()
        {
            self.title = Some(title);
            self.article_url = url;
            return;
        }
        self.begin_select(Some(article_id), None, title, url, cx);
    }

    /// 选中一条本地网页收藏（正文在本地库，没有远端译文）。
    pub fn select_bookmark(
        &mut self,
        bookmark_id: i64,
        title: String,
        url: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.bookmark_id == Some(bookmark_id)
            && self.article_id.is_none()
            && !self.session.state().is_failed()
        {
            self.title = Some(title);
            self.article_url = url;
            return;
        }
        self.begin_select(None, Some(bookmark_id), title, url, cx);
    }

    /// 换一篇正文（文章或收藏），重置提示、滚动位置与推荐状态，然后后台取数。
    fn begin_select(
        &mut self,
        article_id: Option<i64>,
        bookmark_id: Option<i64>,
        title: String,
        url: Option<String>,
        cx: &mut Context<Self>,
    ) {
        // 收藏没有译文，固定原文视图；回到文章时恢复进入收藏前的视图模式。
        if article_id.is_some()
            && self.bookmark_id.is_some()
            && let Some(mode) = self.mode_before_bookmark.take()
        {
            self.session.set_mode(mode);
        }
        if bookmark_id.is_some() && self.bookmark_id.is_none() {
            self.mode_before_bookmark = Some(self.session.mode());
            self.session.set_mode(ViewMode::Original);
        }
        self.article_id = article_id;
        self.bookmark_id = bookmark_id;
        self.title = Some(title);
        self.article_url = url;
        self.translation_notice = None;
        self.link_notice = None;
        self.recommend_dialog = None;
        self.recommend_error = None;
        self.recommend_notice = None;
        self.translation_task = None;
        self.scroll_handle.set_offset(Point {
            x: px(0.),
            y: px(0.),
        });
        if article_id.is_some() {
            self.reload_recommendation();
        } else {
            self.recommendation = None;
        }
        let request = match bookmark_id {
            Some(bookmark_id) => self.data.bookmark_request(bookmark_id),
            None => match article_id {
                Some(article_id) => self.data.chapter_request(article_id),
                None => return,
            },
        };
        match request {
            Ok(request) => {
                self.session.reset_to(request);
                self.rebuild_rendered_rows();
                self.spawn_load(cx);
            }
            Err(error) => {
                // 本地没有正文对象：直接进入失败态，错误页里的“重试”仍有意义。
                self.session.apply_load_result(Err(error));
                self.rebuild_rendered_rows();
                cx.notify();
            }
        }
    }

    /// 清空当前阅读（例如正在读的收藏被移除）。
    ///
    /// 只清选中项与渲染缓存，并递增 generation 让在途请求的结果失效；
    /// 阅读面板随即回到“选择一篇文章开始阅读”的空态。
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.article_id = None;
        self.bookmark_id = None;
        self.mode_before_bookmark = None;
        self.title = None;
        self.article_url = None;
        self.recommendation = None;
        self.recommend_dialog = None;
        self.recommend_error = None;
        self.recommend_notice = None;
        self.translation_notice = None;
        self.link_notice = None;
        self.translation_task = None;
        self.task = None;
        self.rendered_rows.clear();
        let lang = self.session.request().lang.clone();
        self.session
            .reset_to(ChapterRequest::new(String::new(), lang));
        cx.notify();
    }

    /// 重试当前文章（后台取数）。
    ///
    /// 如果上次连 `chapter_request` 都失败了（request 为空），重试会先重新解析
    /// 文章 → 正文 key；这样“服务端还没就绪”之类的暂时性错误恢复后无需重新选文章。
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(bookmark_id) = self.bookmark_id {
            if self.session.request().content_key.is_empty() {
                match self.data.bookmark_request(bookmark_id) {
                    Ok(request) => self.session.reset_to(request),
                    Err(error) => {
                        self.session.mark_retry();
                        self.session.apply_load_result(Err(error));
                        self.rebuild_rendered_rows();
                        cx.notify();
                        return;
                    }
                }
            }
            self.session.mark_retry();
            self.session.begin_load();
            self.rebuild_rendered_rows();
            self.spawn_load(cx);
            cx.notify();
            return;
        }
        let Some(article_id) = self.article_id else {
            return;
        };
        if self.session.request().content_key.is_empty() {
            match self.data.chapter_request(article_id) {
                Ok(request) => self.session.reset_to(request),
                Err(error) => {
                    self.session.mark_retry();
                    self.session.apply_load_result(Err(error));
                    self.rebuild_rendered_rows();
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
        self.rebuild_rendered_rows();
        self.spawn_load(cx);
        cx.notify();
    }

    /// 切换视图；返回是否变化。
    pub fn set_mode(&mut self, mode: ViewMode) -> bool {
        if self.session.mode() == mode {
            return false;
        }
        self.session.set_mode(mode);
        self.rebuild_rendered_rows();
        true
    }

    /// 重建正文渲染缓存（文章 / 视图 / 译文变化后调用）。
    ///
    /// 缓存的是 `SharedString` 而不是 `String`：滚动时同一段 HTML 的分配不变，
    /// 不必每帧重新拷贝整篇正文，`TextView` 也能快速识别文本未变化。
    fn rebuild_rendered_rows(&mut self) {
        self.rendered_rows = self
            .session
            .all_rows()
            .into_iter()
            .map(|row| {
                let original_html = SharedString::from(row.original_html);
                // 缺译文时回退原文；直接复用同一个 SharedString，省一份拷贝。
                let translation_html = match row.translation_html {
                    Some(html) => SharedString::from(html),
                    None => original_html.clone(),
                };
                RenderedRow {
                    index: row.index,
                    tag: row.tag,
                    original_html,
                    translation_html,
                }
            })
            .collect();
    }

    /// PageDown：按视口高度向下连续滚动；已经在文末返回 `false`。
    pub fn page_down(&mut self) -> bool {
        self.scroll_by_page(-1.)
    }

    /// PageUp：按视口高度向上连续滚动；已经在文首返回 `false`。
    pub fn page_up(&mut self) -> bool {
        self.scroll_by_page(1.)
    }

    /// 按视口高度滚动一屏（`-1` 向下、`1` 向上），并夹在正文范围内。
    ///
    /// 桌面阅读面板一次渲染整篇，没有段落分页；PageUp/PageDown 只做纯滚动，
    /// 因此可以一路按到底读完，不会在“页边界”处跳一下或重置阅读位置。
    fn scroll_by_page(&mut self, direction: f32) -> bool {
        let offset = self.scroll_handle.offset();
        let max = self.scroll_handle.max_offset().y.max(px(0.));
        let step = page_step(self.scroll_handle.bounds().size.height) * direction;
        let target_y = (offset.y + step).clamp(-max, px(0.));
        if target_y == offset.y {
            return false;
        }
        self.scroll_handle.set_offset(Point {
            x: offset.x,
            y: target_y,
        });
        true
    }

    /// ↑：向上滚动一行（不跳页）。
    pub fn line_up(&mut self) -> bool {
        self.scroll_lines(1.)
    }

    /// ↓：向下滚动一行（不跳页）。
    pub fn line_down(&mut self) -> bool {
        self.scroll_lines(-1.)
    }

    /// 按当前行高滚动 `lines` 行；方向与 `ScrollHandle` 一致（向下为负）。
    fn scroll_lines(&mut self, lines: f32) -> bool {
        let offset = self.scroll_handle.offset();
        let max = self.scroll_handle.max_offset().y.max(px(0.));
        let delta = self.line_height * lines;
        let target_y = (offset.y + delta).clamp(-max, px(0.));
        if target_y == offset.y {
            return false;
        }
        self.scroll_handle.set_offset(Point {
            x: offset.x,
            y: target_y,
        });
        true
    }

    /// 用系统浏览器打开当前文章的原文；失败时在标题下方提示。
    pub fn open_original(&mut self, cx: &mut Context<Self>) {
        let Some(url) = self.article_url.clone() else {
            return;
        };
        match tela_net::browser::open(&url) {
            Ok(()) => self.link_notice = None,
            Err(error) => self.link_notice = Some(format!("打开原文失败：{error}")),
        }
        cx.notify();
    }

    /// 重新读一次当前文章的推荐状态（取消推荐后由外壳调用）。
    pub fn refresh_recommendation(&mut self, cx: &mut Context<Self>) {
        self.reload_recommendation();
        cx.notify();
    }

    /// 打开推荐弹窗：已推荐时预填推荐语，按钮变为“保存/取消推荐”。
    pub fn open_recommend_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recommendations.is_none() || self.article_id.is_none() {
            return;
        }
        let initial = self
            .recommendation
            .as_ref()
            .and_then(|item| item.note.clone())
            .unwrap_or_default();
        let note = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx);
            state.set_editor_style(InputEditorStyle {
                foreground: rgb(TEXT).into(),
                background: rgb(BG).into(),
                selection: rgba(0x89b4fa55).into(),
                caret: rgb(TEXT).into(),
                ..InputEditorStyle::default()
            });
            state.default_value(initial)
        });
        note.update(cx, |state, cx| state.focus(window, cx));
        self.recommend_dialog = Some(RecommendDialog {
            note,
            editing: self.recommendation.is_some(),
        });
        self.recommend_error = None;
        self.recommend_notice = None;
        cx.notify();
    }

    /// 关闭推荐弹窗（取消、Esc、点遮罩都走这里）。
    pub fn close_recommend_dialog(&mut self) {
        self.recommend_dialog = None;
        self.recommend_error = None;
    }

    /// 提交推荐：读取输入框、校验长度、写本地库并入队，然后通知外壳刷新。
    pub fn submit_recommendation(&mut self, cx: &mut Context<Self>) {
        let Some(article_id) = self.article_id else {
            return;
        };
        let Some(dialog) = &self.recommend_dialog else {
            return;
        };
        let text = dialog.note.read(cx).value().trim().to_string();
        if text.chars().count() > tela_proto::MAX_RECOMMENDATION_NOTE {
            self.recommend_error = Some(format!(
                "推荐语最多 {} 字（当前 {} 字）",
                tela_proto::MAX_RECOMMENDATION_NOTE,
                text.chars().count()
            ));
            cx.notify();
            return;
        }
        let Some(service) = self.recommendations.clone() else {
            self.recommend_error = Some("当前装配不支持推荐".to_string());
            cx.notify();
            return;
        };
        let note = if text.is_empty() { None } else { Some(text) };
        match service.recommend(article_id, note) {
            Ok(()) => {
                self.close_recommend_dialog();
                self.reload_recommendation();
                self.recommend_notice = Some("已推荐。".to_string());
                cx.emit(ReaderEvent::RecommendationChanged { article_id });
            }
            Err(error) => {
                self.recommend_error = Some(format!("推荐失败：{error}"));
            }
        }
        cx.notify();
    }

    /// 在“编辑推荐”弹窗里撤回这条推荐。
    pub fn confirm_unrecommend(&mut self, cx: &mut Context<Self>) {
        let Some(article_id) = self.article_id else {
            return;
        };
        let Some(service) = self.recommendations.clone() else {
            self.recommend_error = Some("当前装配不支持推荐".to_string());
            cx.notify();
            return;
        };
        match service.unrecommend(article_id) {
            Ok(()) => {
                self.close_recommend_dialog();
                self.reload_recommendation();
                self.recommend_notice = Some("已取消推荐。".to_string());
                cx.emit(ReaderEvent::RecommendationChanged { article_id });
            }
            Err(error) => {
                self.recommend_error = Some(format!("取消推荐失败：{error}"));
            }
        }
        cx.notify();
    }

    /// 从数据源重新读取当前文章的推荐行。
    fn reload_recommendation(&mut self) {
        let Some(article_id) = self.article_id else {
            self.recommendation = None;
            return;
        };
        if self.recommendations.is_none() {
            self.recommendation = None;
            return;
        }
        self.recommendation = self.data.recommendation(article_id).ok().flatten();
    }

    /// 键盘快捷键：`o` 原文 / `c` 对照 / `t` 译文 /
    /// `PageUp`/`PageDown` 按视口滚动 / `↑`/`↓` 逐行滚动 / `r` 重试。
    ///
    /// 正文一次渲染整篇，没有段落分页，所以不再有 `n`/`p` 翻页键；
    /// 一直按 `PageDown` 或 `↓` 就能从开头读到结尾。
    ///
    /// 返回是否发生变化；调用方负责 `cx.notify()`。
    pub fn handle_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        // 推荐弹窗打开时，输入框里的字符不能被当成视图/翻页快捷键。
        if self.recommend_dialog.is_some() {
            return false;
        }
        match key {
            "o" => self.set_mode(ViewMode::Original),
            "c" => self.set_mode(ViewMode::Compare),
            "t" => self.set_mode(ViewMode::Translation),
            "up" => self.line_up(),
            "down" => self.line_down(),
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
                    reader.rebuild_rendered_rows();
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
            let mut request_sent = !send_request;
            let mut attempts_used = 0usize;
            let mut offline_notified = false;

            loop {
                // 完全离线：不发 POST，也不查译文状态；只做轻量探活。
                if api.is_offline() {
                    let _ = api.probe_network();
                    let still_current = this
                        .update(cx, |reader, _| {
                            reader.generation == generation && reader.article_id == Some(article_id)
                        })
                        .unwrap_or(false);
                    if !still_current {
                        return;
                    }
                    if api.is_offline() {
                        if !offline_notified {
                            set_translation_notice(
                                &this,
                                cx,
                                generation,
                                article_id,
                                "网络已断开，暂停检查译文；恢复后会自动继续。",
                            );
                            offline_notified = true;
                        }
                        cx.background_executor().timer(OFFLINE_PROBE_INTERVAL).await;
                        continue;
                    }
                    offline_notified = false;
                }

                if !request_sent {
                    request_sent = true;
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
                            TranslationAnswerStatus::Running | TranslationAnswerStatus::Waiting => {
                            }
                            TranslationAnswerStatus::Refused => {
                                let notice = "今日翻译额度已用完，译文明天恢复；现在可以读原文。";
                                set_translation_notice(&this, cx, generation, article_id, notice);
                                return;
                            }
                            TranslationAnswerStatus::Unavailable
                            | TranslationAnswerStatus::Unknown => {
                                let notice = "服务端没有提供这篇的翻译（作者关闭翻译或正文为空）。";
                                set_translation_notice(&this, cx, generation, article_id, notice);
                                return;
                            }
                        },
                        Err(error) => {
                            // 网络断了：保留任务，恢复后重新 POST 一次。
                            if api.is_offline() {
                                request_sent = false;
                                continue;
                            }
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
                if attempts_used >= attempts {
                    break;
                }
                let delay = if attempts_used == 0 {
                    Duration::from_millis(600)
                } else {
                    TRANSLATION_POLL_INTERVAL
                };
                cx.background_executor().timer(delay).await;
                // 等待期间断网：这次不消耗轮询次数，回到顶部进入离线等待。
                if api.is_offline() {
                    continue;
                }
                attempts_used += 1;

                let request_api = Arc::clone(&api);
                let next_request = request.clone();
                // 轮询译文必须绕过本地缓存，否则 pending 快照永远不变；
                // `refresh_chapter` 只查译文状态，不重复下载不可变正文。
                let result = cx
                    .background_executor()
                    .spawn(async move { request_api.refresh_chapter(&next_request) })
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
                    reader.rebuild_rendered_rows();
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
        if !self.has_selection() {
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
        let total_paragraphs = self.session.total_paragraphs();
        // 正文一次渲染整篇，顶部不再出现页码：只留译文状态与段落总数。
        // 收藏的网页没有译文（本地抽取的正文就是原文），改为展示“收藏的网页”。
        let status_line = if self.bookmark_id.is_some() {
            let state = self.session.state().status_label();
            if total_paragraphs > 0 && self.session.state().is_ready() {
                format!("收藏的网页 · 共 {total_paragraphs} 段")
            } else {
                state
            }
        } else {
            let status = self.session.state().status_label();
            if total_paragraphs > 0 {
                format!("{status} · 共 {total_paragraphs} 段")
            } else {
                status
            }
        };
        let mode = self.session.mode();
        let is_bookmark = self.bookmark_id.is_some();

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

        let notice = self.translation_notice.clone();
        let link_notice = self.link_notice.clone();
        let recommend_notice = self.recommend_notice.clone();
        let can_open_original = self.article_url.is_some();
        let show_recommend = self.recommendations.is_some() && self.article_id.is_some();

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
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_lg()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .truncate()
                                            .child(title),
                                    )
                                    .when(can_open_original, |row| {
                                        row.child(
                                            div()
                                                .id("reader-open-original")
                                                .role(Role::Button)
                                                .aria_label("打开原文")
                                                .px_2()
                                                .py_1()
                                                .rounded_md()
                                                .border_1()
                                                .border_color(rgb(BORDER))
                                                .text_sm()
                                                .text_color(rgb(SUBTLE))
                                                .cursor_pointer()
                                                .hover(|style| {
                                                    style.bg(rgb(BORDER)).text_color(rgb(ACCENT))
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.open_original(cx);
                                                }))
                                                .child("↗"),
                                        )
                                    })
                                    .when(show_recommend, |row| {
                                        row.child(self.recommend_button(cx))
                                    }),
                            )
                            .child(div().text_xs().text_color(rgb(SUBTLE)).child(status_line))
                            .when_some(notice, |element, notice| {
                                element.child(div().text_xs().text_color(rgb(WARN)).child(notice))
                            })
                            .when_some(recommend_notice, |element, notice| {
                                element.child(div().text_xs().text_color(rgb(ACCENT)).child(notice))
                            })
                            .when_some(link_notice, |element, notice| {
                                element.child(div().text_xs().text_color(rgb(DANGER)).child(notice))
                            }),
                    )
                    // 收藏的网页没有译文：不显示 o/c/t 视图切换，避免出现两列相同内容。
                    .child(h_flex().items_center().gap_1().children(if is_bookmark {
                        Vec::new()
                    } else {
                        modes
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(if is_bookmark {
                        "Shift+B 继续收藏 · j/k 文章 · ↑/↓ 逐行 · PgUp/PgDn 滚动"
                    } else {
                        "o/c/t 视图 · j/k 文章 · Shift+j/k 博客 · Shift+B 收藏网址 · \
                         ↑/↓ 逐行 · PgUp/PgDn 滚动"
                    }),
            )
    }

    /// 标题旁的“推荐/已推荐”按钮：点击打开填写推荐理由的弹窗。
    fn recommend_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let recommended = self.recommendation.is_some();
        let label = if recommended { "已推荐" } else { "推荐" };
        div()
            .id("reader-recommend")
            .role(Role::Button)
            .aria_label(if recommended {
                "修改或取消推荐"
            } else {
                "推荐这篇文章"
            })
            .px_3()
            .py_1()
            .rounded_full()
            .border_1()
            .border_color(rgb(if recommended { ACCENT } else { BORDER }))
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .when(recommended, |element| {
                element.bg(rgb(ACCENT)).text_color(rgb(BG))
            })
            .when(!recommended, |element| {
                element
                    .text_color(rgb(SUBTLE))
                    .hover(|style| style.bg(rgb(BORDER)).text_color(rgb(ACCENT)))
            })
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_recommend_dialog(window, cx);
            }))
            .child(label)
            .into_any_element()
    }

    fn body(&self, cx: &mut Context<Self>) -> Div {
        if !self.has_selection() {
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
        // 整篇一次渲染：不分页、不切页，一直按 PgDn / ↓ 就能读到结尾。
        // 行文本用缓存好的 SharedString，滚动时不会重新解析 HTML。
        let rows = &self.rendered_rows;
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
                    .child("这篇文章没有可显示的段落。"),
            );
        } else {
            list = list.child(self.column_header(mode));
            for row in rows {
                list = list.child(self.row_block(row, mode));
            }
            // 结尾标记：给读者一个明确的“读完了”信号，避免以为还有内容没加载。
            list = list.child(
                div()
                    .w_full()
                    .max_w(px(READING_MAX_WIDTH))
                    .pt_8()
                    .pb_4()
                    .text_xs()
                    .text_center()
                    .text_color(rgb(MUTED))
                    .child("— 全文完 —"),
            );
        }
        v_flex().flex_1().min_h_0().child(list)
    }

    /// 顶部只出现一次的列标题（原文 / 译文）。
    fn column_header(&self, mode: ViewMode) -> Div {
        let Some(doc) = self.session.state().document() else {
            return div().w_full();
        };
        // 本地收藏的网页可能没有声明语言：只显示「原文」，不留一个空的「· 」。
        let original = if doc.source_lang.trim().is_empty() {
            "原文".to_string()
        } else {
            format!("原文 · {}", language_label(&doc.source_lang))
        };
        let translated = if doc.lang.trim().is_empty() {
            "译文".to_string()
        } else {
            format!("译文 · {}", language_label(&doc.lang))
        };

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

    fn row_block(&self, row: &RenderedRow, mode: ViewMode) -> Div {
        let original = self.block_view(
            format!("reader-original-{}", row.index),
            row.original_html.clone(),
            &row.tag,
        );

        // 缺译文时回退原文（代码/图片/超额段落仍然可读）；顶部状态条会说明
        // 整篇存在未翻译段落，不在每段中间插占位句。
        let translation = self.block_view(
            format!("reader-translation-{}", row.index),
            row.translation_html.clone(),
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
    fn block_view(&self, id: String, html: SharedString, tag: &str) -> AnyElement {
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
    fn rich_text(&self, id: String, html: SharedString) -> impl IntoElement {
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

    /// 推荐弹窗（窗口级模态）：标题、推荐语输入框、提示与按钮。
    fn recommend_dialog_layer(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = &self.recommend_dialog else {
            return div().into_any_element();
        };
        let editing = dialog.editing;
        let note = dialog.note.clone();
        let focus_note = note.clone();
        let error = self.recommend_error.clone();

        let textarea = InputBase::new("reader-recommend-note")
            .w_full()
            .h(px(132.))
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                focus_note.update(cx, |state, cx| state.focus(window, cx));
            })
            .child(div().size_full().child(Textarea::new(&note)));

        let cancel = div()
            .id("reader-recommend-cancel")
            .role(Role::Button)
            .aria_label("取消")
            .px_3()
            .py_2()
            .rounded_full()
            .border_1()
            .border_color(rgb(BORDER))
            .text_sm()
            .text_color(rgb(SUBTLE))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(BORDER)).text_color(rgb(TEXT)))
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_recommend_dialog();
                cx.notify();
            }))
            .child("取消");

        let submit = div()
            .id("reader-recommend-submit")
            .role(Role::Button)
            .aria_label(if editing { "保存推荐" } else { "推荐" })
            .px_4()
            .py_2()
            .rounded_full()
            .bg(rgb(ACCENT))
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(BG))
            .cursor_pointer()
            .hover(|style| style.opacity(0.85))
            .on_click(cx.listener(|this, _, _, cx| this.submit_recommendation(cx)))
            .child(if editing { "保存" } else { "推荐" });

        let unrecommend = div()
            .id("reader-recommend-unrecommend")
            .role(Role::Button)
            .aria_label("取消推荐")
            .px_3()
            .py_2()
            .rounded_full()
            .text_sm()
            .text_color(rgb(DANGER))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(DANGER)).text_color(rgb(BG)))
            .on_click(cx.listener(|this, _, _, cx| this.confirm_unrecommend(cx)))
            .child("取消推荐");

        let close_this = cx.entity().downgrade();
        let submit_this = cx.entity().downgrade();
        let popup = DialogPopup::new()
            .w(px(540.))
            .p_5()
            .gap_3()
            .flex()
            .flex_col()
            .items_stretch()
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(PANEL))
            .text_color(rgb(TEXT))
            .child(
                DialogTitle::new()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if editing {
                        "编辑推荐"
                    } else {
                        "推荐给你的读者"
                    }),
            )
            .child(
                DialogDescription::new()
                    .mt_1()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("推荐语会显示在你的个人页，作者也能看到；可以留空。"),
            )
            .child(div().mt_3().child(textarea))
            .when_some(error, |element, error| {
                element.child(div().mt_2().text_xs().text_color(rgb(DANGER)).child(error))
            })
            .child(
                div()
                    .mt_2()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(format!("最多 {} 字", tela_proto::MAX_RECOMMENDATION_NOTE)),
            )
            .child(
                h_flex()
                    .mt_4()
                    .items_center()
                    .justify_between()
                    .child(if editing {
                        unrecommend.into_any_element()
                    } else {
                        div().into_any_element()
                    })
                    .child(h_flex().gap_2().child(cancel).child(submit)),
            );

        Dialog::new(cx)
            .open(true)
            // 焦点在输入框时 Enter 由 Textarea 自己处理（换行）；焦点在按钮上时
            // Enter 等同提交。提交失败要保留弹窗，所以这里不让 Dialog 自行关闭。
            .on_ok(move |_, _, cx| {
                let _ = submit_this.update(cx, |reader, cx| reader.submit_recommendation(cx));
                false
            })
            .on_open_change(move |open, _, _, cx| {
                if open {
                    return;
                }
                let _ = close_this.update(cx, |reader, cx| {
                    reader.close_recommend_dialog();
                    cx.notify();
                });
            })
            .backdrop(
                DialogBackdrop::new()
                    .absolute()
                    .inset_0()
                    .bg(rgba(0x000000aa)),
            )
            .popup(popup)
            .into_any_element()
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
        // Up/Down 按窗口正文行高滚动；放大字体时步长跟着变。
        self.line_height = window.line_height();
        let header = self.header(cx);
        let body = self.body(cx);
        let recommend_dialog = self.recommend_dialog_layer(cx);
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
            .child(recommend_dialog)
    }
}

impl EventEmitter<ReaderEvent> for ReaderView {}

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
