//! 正文页渲染（切片 M3-UI-3）：把阅读会话的当前状态渲染成可直接显示/打印的行。
//!
//! 契约来源：docs/development_plan.md §6 C11（正文按段落渲染 + 三视图切换）、
//! §9 M3（加载 / 空 / 错误态、长章节分页、切换视图不丢阅读位置）；
//! docs/product-design.md §7.4。
//!
//! 为什么渲染在 `tela-ui` 而不是 app：无显示器环境下（CI、本机缺 GUI 系统库）
//! 阅读页仍必须可验证，所以「页面长什么样」收敛成一个纯函数（会话 → 行），
//! GUI 组件与终端入口共用同一份渲染逻辑，不会出现两套视觉/两套行为。

use crate::reader::session::ReaderSession;

/// 页头与正文之间的分隔线（同时充当「页面已渲染」的可见证据）。
pub const PAGE_RULE: &str = "────────────────────────────";

/// 原文列的行内前缀。
pub const ORIGINAL_COLUMN: &str = "原文：";

/// 译文列的行内前缀。
pub const TRANSLATION_COLUMN: &str = "译文：";

/// 译文视图下缺译段落的回退前缀（显式说明这是原文，避免误认为译文）。
pub const TRANSLATION_FALLBACK_COLUMN: &str = "译文（原文回退）：";

/// 把当前阅读会话渲染成一页文本行。
///
/// 结构：`[状态栏] / [分隔线] / [段落块…]`；每段先出段号+标签，再按当前视图
/// 输出原文列、译文列（对照视图两列都出），因此段落不会错位，也不会出现空行。
pub fn render_session(session: &ReaderSession) -> Vec<String> {
    let mut lines = Vec::with_capacity(session.visible_rows().len() * 3 + 2);
    lines.push(status_bar(session));
    lines.push(PAGE_RULE.to_string());

    for row in session.visible_rows() {
        lines.push(format!("[{:02}] {}", row.index + 1, row.tag));
        if row.show_original {
            lines.push(format!("{ORIGINAL_COLUMN}{}", row.original));
        }
        if row.show_translation {
            if row.translation_missing {
                // 缺失段落回退原文；显式标出，终端里不会误以为这是译文。
                lines.push(format!("{TRANSLATION_FALLBACK_COLUMN}{}", row.translation));
            } else {
                lines.push(format!("{TRANSLATION_COLUMN}{}", row.translation));
            }
        }
    }

    lines
}

/// 顶部状态栏：状态（四态之一）+ 视图 + 页码 + 段数，一行给全。
fn status_bar(session: &ReaderSession) -> String {
    format!(
        "Tela 正文 · {} · 视图：{} · 第 {}/{} 页 · 共 {} 段",
        session.state().status_label(),
        session.mode().label(),
        session.page() + 1,
        session.page_count(),
        session.total_paragraphs(),
    )
}
