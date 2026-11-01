//! 阅读视图（M3）：正文按段落渲染 + 译文逐段对照 + 三视图切换。
//!
//! 职责边界（docs/development_plan.md §5）：只消费 `tela-content` 的 `Chapter`，
//! 不直接发 HTTP、不持有 mock 数据；三种视图（原文 / 对照 / 译文）与加载态、
//! 空章节、失败重试、按需分页都收敛在这一层，便于无显示器环境下用纯逻辑测试。
//! 桌面端一次取整篇连续滚动（[`ReaderSession::all_rows`]），终端仍可逐页消费
//! （[`ReaderSession::visible_rows`]），两者共用同一份段落对齐与视图语义。

pub mod document;
pub mod page;
pub mod session;

pub use document::{ReaderDocument, ReaderParagraph, ReaderRow, html_to_text};
pub use page::{PAGE_RULE, render_session};
pub use session::{DEFAULT_PAGE_SIZE, EMPTY_HINT, ReaderSession, ReaderState};

/// 阅读区的三种视图（docs/product-design.md §7.4 模式切换）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// 原文：单栏，仅原文。
    Original,
    /// 对照：左原文右译文，块级对齐。
    #[default]
    Compare,
    /// 译文：单栏，译文优先；缺译文回退原文。
    Translation,
}

impl ViewMode {
    /// 三种视图的固定顺序（与产品文档 `[原文 | 对照 | 译文]` 一致）。
    pub const ALL: [ViewMode; 3] = [Self::Original, Self::Compare, Self::Translation];

    /// 稳定的 wire/CLI 标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Compare => "compare",
            Self::Translation => "translation",
        }
    }

    /// 界面按钮文案。
    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "原文",
            Self::Compare => "对照",
            Self::Translation => "译文",
        }
    }

    /// 解析 CLI/env 取值；兼容计划里的 `orig` / `side` / `trans` 简称。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "original" | "orig" => Some(Self::Original),
            "compare" | "side" | "dual" => Some(Self::Compare),
            "translation" | "trans" | "translated" => Some(Self::Translation),
            _ => None,
        }
    }
}
