//! 阅读会话状态机（切片 M3-UI-2 起）：加载 / 空章节 / 失败重试 / 视图 / 阅读位置。
//!
//! 契约来源：docs/development_plan.md §6 C11（正文按段落渲染 + 三视图切换）、
//! §9 M3（加载/空/错误态、长章节分页、切换视图不丢阅读位置）；
//! docs/product-design.md §7.4。
//!
//! 边界：本层只用 [`ContentApi`] 取数（mock 与真实 HTTP 同一 trait），
//! 不持有任何正文 fixture，也不直接绘制。

use std::sync::Arc;

use tela_content::api::{ChapterRequest, ContentApi};
use tela_content::model::Chapter;
use tela_proto::TelaError;

use crate::reader::ViewMode;
use crate::reader::document::{ReaderDocument, ReaderRow, TranslationStatus};

/// 一页默认渲染的段落数（长章节分页 / 懒加载的页大小）。
pub const DEFAULT_PAGE_SIZE: usize = 20;

/// 空章节的界面提示（不是错误）。
pub const EMPTY_HINT: &str = "本章没有可显示的正文内容";

/// 阅读区的四态：加载中 / 空章节 / 就绪 / 失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderState {
    /// 请求进行中。
    Loading,
    /// 请求成功但章节为空（空章节不是错误）。
    Empty,
    /// 请求成功且有正文。
    Ready(ReaderDocument),
    /// 请求失败（含可重试提示）。
    Failed {
        /// 失败原因（三段式文案，直接展示给用户）。
        message: String,
        /// 已重试次数。
        retries: usize,
    },
}

impl ReaderState {
    /// 是否加载中。
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    /// 是否空章节。
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// 是否失败。
    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    /// 是否就绪。
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    /// 就绪时的阅读文档。
    pub fn document(&self) -> Option<&ReaderDocument> {
        match self {
            Self::Ready(doc) => Some(doc),
            _ => None,
        }
    }

    /// 失败原因。
    pub fn error_message(&self) -> Option<&str> {
        match self {
            Self::Failed { message, .. } => Some(message),
            _ => None,
        }
    }

    /// 顶部状态说明（四态统一入口）。
    pub fn status_label(&self) -> String {
        match self {
            Self::Loading => "加载中…".to_string(),
            Self::Empty => EMPTY_HINT.to_string(),
            Self::Ready(doc) => match doc.translation_status {
                TranslationStatus::Partial => {
                    format!(
                        "{}（{} 段显示原文）",
                        doc.translation_status.label(),
                        doc.gap_count()
                    )
                }
                _ => doc.translation_status.label().to_string(),
            },
            Self::Failed { message, .. } => format!("加载失败：{message}（按 r 重试）"),
        }
    }
}

/// 一次阅读会话：持有取数 API、视图模式、分页与阅读位置。
pub struct ReaderSession {
    api: Arc<dyn ContentApi>,
    request: ChapterRequest,
    state: ReaderState,
    mode: ViewMode,
    page_size: usize,
    /// 阅读位置（锚点）：当前页首段的段落索引；切换视图 / 重试后保持不变。
    anchor: usize,
    /// 已重试次数（失败态展示用）。
    retries: usize,
}

impl ReaderSession {
    /// 新建会话：初始为 [`ReaderState::Loading`]，默认对照视图。
    pub fn new(api: Arc<dyn ContentApi>, request: ChapterRequest) -> Self {
        Self {
            api,
            request,
            state: ReaderState::Loading,
            mode: ViewMode::Compare,
            page_size: DEFAULT_PAGE_SIZE,
            anchor: 0,
            retries: 0,
        }
    }

    /// 设置页大小（`0` 退回默认值）。
    pub fn with_page_size(mut self, page_size: usize) -> Self {
        self.page_size = if page_size == 0 {
            DEFAULT_PAGE_SIZE
        } else {
            page_size
        };
        self
    }

    /// 当前请求（GUI 在后台任务里取数时用）。
    pub fn request(&self) -> &ChapterRequest {
        &self.request
    }

    /// 换一篇正文并回到加载态、阅读位置归零。
    ///
    /// GUI 的加载流程是「先 reset_to → 后台取数 → apply_load_result」，
    /// 与终端宿主的 `load()` 共用同一套状态机，不产生第二套四态语义。
    pub fn reset_to(&mut self, request: ChapterRequest) {
        self.request = request;
        self.state = ReaderState::Loading;
        self.anchor = 0;
        self.retries = 0;
    }

    /// 进入加载态（保留当前请求与阅读位置，供重试/刷新使用）。
    pub fn begin_load(&mut self) {
        self.state = ReaderState::Loading;
    }

    /// 记录一次重试（失败页展示重试次数用）。
    pub fn mark_retry(&mut self) {
        self.retries += 1;
    }

    /// 应用一次异步取数的结果；GUI 与终端共用同一套映射。
    pub fn apply_load_result(&mut self, result: Result<Chapter, TelaError>) {
        self.apply(result);
    }

    /// 发起取数并把结果映射成四态（空章节 → `Empty`，失败 → `Failed`）。
    pub fn load(&mut self) {
        let result = self.api.load_chapter(&self.request);
        self.apply(result);
    }

    /// 重试：重新取数（失败计数 +1，成功后清空错误）。
    pub fn retry(&mut self) {
        self.mark_retry();
        self.load();
    }

    /// 把 API 结果映射成四态；失败时不保留旧内容（避免错误态叠着旧正文）。
    fn apply(&mut self, result: Result<Chapter, TelaError>) {
        match result {
            Ok(chapter) if chapter.is_empty() => {
                self.state = ReaderState::Empty;
            }
            Ok(chapter) => {
                let document = ReaderDocument::from_chapter(&self.request, &chapter);
                self.anchor =
                    clamp_anchor(self.anchor, document.total_paragraphs(), self.page_size);
                self.state = ReaderState::Ready(document);
            }
            Err(error) => {
                self.state = ReaderState::Failed {
                    message: error.to_string(),
                    retries: self.retries,
                };
            }
        }
    }

    /// 当前状态。
    pub fn state(&self) -> &ReaderState {
        &self.state
    }

    /// 当前视图模式。
    pub fn mode(&self) -> ViewMode {
        self.mode
    }

    /// 切换视图模式（不改变阅读位置）。
    pub fn set_mode(&mut self, mode: ViewMode) {
        self.mode = mode;
    }

    /// 页大小。
    pub fn page_size(&self) -> usize {
        self.page_size
    }

    /// 阅读位置（当前页首段索引）。
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// 已重试次数。
    pub fn retries(&self) -> usize {
        self.retries
    }

    /// 当前章节的总段落数（非就绪态为 0）。
    pub fn total_paragraphs(&self) -> usize {
        self.state
            .document()
            .map(ReaderDocument::total_paragraphs)
            .unwrap_or(0)
    }

    /// 总页数（非就绪态视为 1 页，避免除零；末页允许不满页）。
    pub fn page_count(&self) -> usize {
        let total = self.total_paragraphs();
        if total == 0 || self.page_size == 0 {
            1
        } else {
            total.div_ceil(self.page_size)
        }
    }

    /// 当前页序号（从 0 开始）。
    pub fn page(&self) -> usize {
        self.anchor.checked_div(self.page_size).unwrap_or(0)
    }

    /// 下翻一页；已在末页返回 `false`（不越界）。
    pub fn next_page(&mut self) -> bool {
        if self.page() + 1 >= self.page_count() {
            return false;
        }
        self.anchor = clamp_anchor(
            self.anchor + self.page_size,
            self.total_paragraphs(),
            self.page_size,
        );
        true
    }

    /// 上翻一页；已在首页返回 `false`（不越界）。
    pub fn prev_page(&mut self) -> bool {
        if self.page() == 0 {
            return false;
        }
        self.anchor = self.anchor.saturating_sub(self.page_size);
        true
    }

    /// 跳到指定页（越界自动夹到合法页，锚点对齐页首）。
    pub fn set_page(&mut self, page: usize) {
        let last = self.page_count().saturating_sub(1);
        let target = page.min(last);
        self.anchor = clamp_anchor(
            target * self.page_size,
            self.total_paragraphs(),
            self.page_size,
        );
    }

    /// 按段落索引定位（落到该段所在页，锚点对齐该页页首，避免半页错位）。
    pub fn goto_paragraph(&mut self, index: usize) {
        let total = self.total_paragraphs();
        if total == 0 || self.page_size == 0 {
            self.anchor = 0;
            return;
        }
        let index = index.min(total - 1);
        self.anchor = (index / self.page_size) * self.page_size;
    }

    /// 当前页的渲染行（终端分页用）。
    pub fn visible_rows(&self) -> Vec<ReaderRow> {
        match &self.state {
            ReaderState::Ready(doc) => doc.rows(self.mode, self.anchor, self.page_size),
            _ => Vec::new(),
        }
    }

    /// 整篇的渲染行（桌面端连续滚动阅读用，不分页）。
    ///
    /// 与 [`Self::visible_rows`] 交付的段落内容、顺序和索引完全一致，只是
    /// 不按 `page_size` 切片：调用方一次拿到全文，用户一直按 PgDn / ↓
    /// 就能读完，不需要“上一页 / 下一页”。
    pub fn all_rows(&self) -> Vec<ReaderRow> {
        match &self.state {
            ReaderState::Ready(doc) => doc.all_rows(self.mode),
            _ => Vec::new(),
        }
    }
}

/// 把锚点夹到合法页首（`total == 0` 时为 0），保证翻页/跳转不越界。
fn clamp_anchor(anchor: usize, total: usize, page_size: usize) -> usize {
    if total == 0 || page_size == 0 {
        return 0;
    }
    let last_page_start = ((total - 1) / page_size) * page_size;
    anchor.min(last_page_start)
}
