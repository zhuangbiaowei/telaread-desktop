//! 收藏网页弹窗（`Shift+B`）：输入网址 → 后台抓取页面 → 写入「我的收藏」。
//!
//! 与阅读面板的推荐弹窗一样是窗口级模态：弹窗打开时外壳不再处理 `j/k` 等快捷键，
//! 输入框独占键盘。下载在后台执行器里跑，**不阻塞渲染线程**；下载完成/失败都在
//! 弹窗里给出反馈，成功后通过 [`BookmarkDialogEvent::Saved`] 通知外壳刷新列表。
//!
//! 边界：本视图只做「输入 + 触发 + 提示」，抓取与落库都在
//! [`tela_ui::shell::BookmarkService`] 的实现里（`tela-app`）。

use std::sync::Arc;

use crate::shell::{BookmarkItem, BookmarkService};

use gpui_kit::base::input::{InputEditorStyle, InputState};
use gpui_kit::base::{
    Dialog, DialogBackdrop, DialogDescription, DialogPopup, DialogTitle, Input, InputBase, h_flex,
};
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

/// 收藏弹窗对外的通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BookmarkDialogEvent {
    /// 收藏成功（标题、正文与图片都已落库）。
    Saved(BookmarkItem),
    /// 用户取消（Esc / 点遮罩 / 「取消」按钮）。
    Cancelled,
}

/// 收藏网页弹窗视图。
pub struct BookmarkDialogView {
    url: Entity<InputState>,
    service: Option<Arc<dyn BookmarkService>>,
    /// 正在下载：禁止重复提交，并显示“正在下载…”。
    busy: bool,
    /// 上一次失败的说明（保留弹窗，方便改地址后重试）。
    error: Option<String>,
    /// 后台下载任务；必须保活。
    task: Option<Task<()>>,
}

impl BookmarkDialogView {
    /// 新建弹窗：立即聚焦输入框，并预填当前剪贴板里的网址（如果像网址）。
    pub fn new(
        service: Option<Arc<dyn BookmarkService>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let clipboard_url = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .map(|text| text.trim().to_string())
            .filter(|text| looks_like_url(text));

        let url = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("https://example.com/article");
            state.set_editor_style(InputEditorStyle {
                foreground: rgb(TEXT).into(),
                background: rgb(BG).into(),
                selection: rgba(0x89b4fa55).into(),
                caret: rgb(TEXT).into(),
                ..InputEditorStyle::default()
            });
            if let Some(url) = clipboard_url {
                state = state.default_value(url);
            }
            state
        });
        url.update(cx, |state, cx| state.focus(window, cx));

        Self {
            url,
            service,
            busy: false,
            error: None,
            task: None,
        }
    }

    /// 当前输入框里的地址（测试/诊断用）。
    pub fn url_text(&self, cx: &App) -> String {
        self.url.read(cx).value().trim().to_string()
    }

    /// 是否正在下载（测试/诊断用）。
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// 上一次失败的提示（测试/诊断用）。
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// 提交：校验输入 → 后台下载 → 成功发事件 / 失败留在弹窗里。
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let url = self.url.read(cx).value().trim().to_string();
        if url.is_empty() {
            self.error = Some("请输入要收藏的网址".to_string());
            cx.notify();
            return;
        }
        let Some(service) = self.service.clone() else {
            self.error = Some("当前装配不支持收藏".to_string());
            cx.notify();
            return;
        };

        self.busy = true;
        self.error = None;
        cx.notify();

        let task = cx.spawn(async move |this, cx: &mut AsyncApp| {
            // 网络 + 磁盘都放在后台执行器：下载大页面/图片时窗口仍然可操作。
            let result = cx
                .background_executor()
                .spawn(async move { service.add_bookmark(&url) })
                .await;
            let _ = this.update(cx, |dialog, cx| {
                dialog.busy = false;
                match result {
                    Ok(item) => cx.emit(BookmarkDialogEvent::Saved(item)),
                    Err(error) => {
                        dialog.error = Some(format!("收藏失败：{error}"));
                        cx.notify();
                    }
                }
            });
        });
        self.task = Some(task);
    }

    /// 取消（Esc / 遮罩 / 按钮）。
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(BookmarkDialogEvent::Cancelled);
    }

    fn dialog_layer(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy;
        let error = self.error.clone();
        let focus_url = self.url.clone();

        let input = InputBase::new("bookmark-url")
            .w_full()
            .h(px(40.))
            .px_3()
            .rounded_md()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                focus_url.update(cx, |state, cx| state.focus(window, cx));
            })
            .child(div().size_full().child(Input::new(&self.url)));

        let cancel = div()
            .id("bookmark-cancel")
            .role(Role::Button)
            .aria_label("取消收藏")
            .px_3()
            .py_2()
            .rounded_full()
            .border_1()
            .border_color(rgb(BORDER))
            .text_sm()
            .text_color(rgb(SUBTLE))
            .cursor_pointer()
            .when(!busy, |element| {
                element
                    .hover(|style| style.bg(rgb(BORDER)).text_color(rgb(TEXT)))
                    .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
            })
            .child("取消");

        let submit = div()
            .id("bookmark-submit")
            .role(Role::Button)
            .aria_label("下载并收藏")
            .px_4()
            .py_2()
            .rounded_full()
            .bg(rgb(if busy { BORDER } else { ACCENT }))
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(if busy { MUTED } else { BG }))
            .when(!busy, |element| {
                element
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.85))
                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
            })
            .child(if busy { "正在下载…" } else { "收藏" });

        let close_this = cx.entity().downgrade();
        let submit_this = cx.entity().downgrade();
        let popup = DialogPopup::new()
            .w(px(560.))
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
                    .child("收藏网页"),
            )
            .child(
                DialogDescription::new()
                    .mt_1()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(
                        "输入一个网址（Shift+B 打开），Tela 会下载页面正文、标题和图片，\
                         保存到左侧「我的收藏」，断网也能读。",
                    ),
            )
            .child(div().mt_3().child(input))
            .when_some(error, |element, error| {
                element.child(div().mt_2().text_xs().text_color(rgb(DANGER)).child(error))
            })
            .when(busy, |element| {
                element.child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(rgb(WARN))
                        .child("正在下载页面与图片，请稍候…（大图较多的页面需要几秒）"),
                )
            })
            .child(
                h_flex()
                    .mt_4()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("Esc 取消 · Enter 收藏"),
                    )
                    .child(h_flex().gap_2().child(cancel).child(submit)),
            );

        Dialog::new(cx)
            .open(true)
            // 下载期间不允许 Esc/点遮罩取消：后台任务已经发出，关掉弹窗会让成功/失败
            // 结果无处展示（用户会以为没保存）。等这一轮结束再关。
            .close_on_escape(!busy)
            .close_on_backdrop_press(!busy)
            // 下载失败要保留弹窗（地址可改），所以确认后不自动关闭。
            .on_ok(move |_, _, cx| {
                let _ = submit_this.update(cx, |dialog, cx| dialog.submit(cx));
                false
            })
            .on_open_change(move |open, _, _, cx| {
                if open {
                    return;
                }
                let _ = close_this.update(cx, |dialog, cx| dialog.cancel(cx));
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
}

impl Render for BookmarkDialogView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 弹窗层是 `deferred` + `anchored` 的绝对定位元素：不参与外壳的布局，
        // 因此这里直接返回弹窗本身，不额外包一层会占位的容器。
        self.dialog_layer(cx)
    }
}

impl EventEmitter<BookmarkDialogEvent> for BookmarkDialogView {}

/// 粗略判断一段文本像不像网址（只用于决定是否预填剪贴板内容）。
fn looks_like_url(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return false;
    }
    text.starts_with("http://")
        || text.starts_with("https://")
        || (text.contains('.') && !text.starts_with('.'))
}
