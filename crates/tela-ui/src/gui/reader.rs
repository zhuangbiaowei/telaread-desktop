//! 阅读面板（S3）：把 [`ReaderSession`] 接到 GPUI，负责异步取数、三视图、翻页与重试。
//!
//! 关键边界：
//! - 正文取数在后台执行器里跑，**不阻塞渲染线程**；
//! - 语义仍由 [`ReaderSession`] 决定（段落对齐、三视图、阅读位置），GUI 只画 `ReaderRow`；
//! - 每次加载带 generation，切文章时旧请求的结果会被丢弃，避免“慢请求覆盖新文章”。

use std::sync::Arc;

use tela_content::api::{ChapterRequest, ContentApi};

use crate::reader::session::{ReaderSession, ReaderState};
use crate::reader::{ReaderRow, ViewMode};
use crate::shell::ShellData;

use gpui_kit::base::{h_flex, v_flex};
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

    /// 下一页；返回是否变化。
    pub fn next_page(&mut self) -> bool {
        self.session.next_page()
    }

    /// 上一页；返回是否变化。
    pub fn prev_page(&mut self) -> bool {
        self.session.prev_page()
    }

    /// 键盘快捷键：`o` 原文 / `c` 对照 / `t` 译文 / `n` 下一页 / `p` 上一页 / `r` 重试。
    ///
    /// 返回是否发生变化；调用方负责 `cx.notify()`。
    pub fn handle_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        match key {
            "o" => self.set_mode(ViewMode::Original),
            "c" => self.set_mode(ViewMode::Compare),
            "t" => self.set_mode(ViewMode::Translation),
            "n" => self.next_page(),
            "p" => self.prev_page(),
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
                    cx.notify();
                }
            });
        });
        self.task = Some(task);
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
                                    .child(format!("{status} · 第 {page}/{pages} 页")),
                            ),
                    )
                    .child(h_flex().items_center().gap_1().children(modes)),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                        "共 {} 段 · 键盘 o/c/t 切换视图，n/p 翻页，r 重试",
                        self.session.total_paragraphs()
                    )))
                    .child(
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
                    ),
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
            .overflow_y_scroll()
            .p_6()
            .gap_4();

        if rows.is_empty() {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("这一页没有可显示的段落。"),
            );
        } else {
            for row in &rows {
                list = list.child(Self::row_block(row, mode));
            }
        }
        v_flex().flex_1().min_h_0().child(list)
    }

    fn row_block(row: &ReaderRow, mode: ViewMode) -> Div {
        let heading = row.tag.starts_with('h');
        let text_size = if heading {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        };

        let original = v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(format!("原文 · {}", row.tag)),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(text_size)
                    .line_height(relative(1.6))
                    .child(row.original.clone()),
            );

        let translation = v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(if row.translation_missing {
                        rgb(DANGER)
                    } else {
                        rgb(MUTED)
                    })
                    .child(if row.translation_missing {
                        "译文 · 缺失"
                    } else {
                        "译文"
                    }),
            )
            .child(
                div()
                    .text_sm()
                    .line_height(relative(1.6))
                    .text_color(if row.translation_missing {
                        rgb(SUBTLE)
                    } else {
                        rgb(TEXT)
                    })
                    .child(row.translation.clone()),
            );

        let content = match mode {
            ViewMode::Original => h_flex().w_full().child(original),
            ViewMode::Translation => h_flex().w_full().child(translation),
            ViewMode::Compare => h_flex()
                .w_full()
                .items_start()
                .gap_6()
                .child(original)
                .child(translation),
        };

        v_flex()
            .w_full()
            .pb_4()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(content)
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
