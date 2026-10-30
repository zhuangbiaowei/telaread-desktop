//! 切片 M3-UI-3：正文页渲染（会话状态 → 可显示的行）。
//!
//! 契约来源：docs/development_plan.md §6 C11、§9 M3；docs/product-design.md §7.4。
//!
//! 断言真实渲染结果：正文/译文全部来自 `tela_content` 的 mock API，
//! 页面渲染只消费 [`ReaderSession`]，不持有任何正文数据。

use std::sync::Arc;

use tela_content::api::ChapterRequest;
use tela_content::mock::{
    MOCK_CONTENT_KEY, MOCK_EMPTY_CONTENT_KEY, MOCK_LONG_CONTENT_KEY, MOCK_PARTIAL_CONTENT_KEY,
    MockContentApi,
};
use tela_ui::reader::ViewMode;
use tela_ui::reader::page::render_session;
use tela_ui::reader::session::{EMPTY_HINT, ReaderSession};

fn session_with(api: MockContentApi, key: &str) -> ReaderSession {
    ReaderSession::new(Arc::new(api), ChapterRequest::new(key, "zh-Hans"))
}

/// 渲染结果里不允许出现空行（否则界面会出现常驻空白）。
fn assert_no_blank_lines(lines: &[String]) {
    assert!(!lines.is_empty(), "渲染结果不能是空的（页面没有任何行）");
    for (i, line) in lines.iter().enumerate() {
        assert!(
            !line.trim().is_empty(),
            "第 {i} 行是空行，渲染结果不允许空白行：{lines:#?}"
        );
    }
}

/// 以某前缀开头的行（列内容行）。
fn lines_with_prefix<'a>(lines: &'a [String], prefix: &str) -> Vec<&'a str> {
    lines
        .iter()
        .map(String::as_str)
        .filter(|line| line.starts_with(prefix))
        .collect()
}

/// 对照视图：原文与译文逐段同排（同一段紧邻），页头给出视图/页码/段数。
#[test]
fn compare_view_renders_paragraph_pairs_in_aligned_order() {
    let mut session = session_with(MockContentApi::new(), MOCK_CONTENT_KEY);
    session.load();

    let lines = render_session(&session);
    assert_no_blank_lines(&lines);

    let header = &lines[0];
    assert!(
        header.contains("视图：对照") && header.contains("第 1/1 页") && header.contains("共 5 段"),
        "页头必须给出视图、页码与段数，实际 {header:?}"
    );

    // 每段原文的下一行必须是同一段的译文（不错位、不错行）。
    for (original, translation) in [
        ("原文：Hello Tela", "译文：你好，Tela"),
        ("原文：Reading is a conversation.", "译文：阅读是一场对话。"),
    ] {
        let pos = lines
            .iter()
            .position(|line| line == original)
            .unwrap_or_else(|| panic!("渲染结果缺少原文行 {original:?}：{lines:#?}"));
        assert_eq!(
            lines.get(pos + 1).map(String::as_str),
            Some(translation),
            "{original:?} 的下一行必须是同一段的译文，实际 {:?}",
            lines.get(pos + 1)
        );
    }

    assert_eq!(
        lines_with_prefix(&lines, "原文：").len(),
        5,
        "5 段原文必须全部渲染：{lines:#?}"
    );
    assert_eq!(
        lines_with_prefix(&lines, "译文：").len(),
        5,
        "5 段译文必须全部渲染（完整译文场景）：{lines:#?}"
    );
}

/// 原文视图：只剩原文列，页码/段数不变。
#[test]
fn original_view_hides_translation_column() {
    let mut session = session_with(MockContentApi::new(), MOCK_CONTENT_KEY);
    session.load();
    session.set_mode(ViewMode::Original);

    let lines = render_session(&session);
    assert_no_blank_lines(&lines);
    assert!(
        lines[0].contains("视图：原文"),
        "页头应显示当前视图，实际 {:?}",
        lines[0]
    );
    assert_eq!(lines_with_prefix(&lines, "原文：").len(), 5);
    assert!(
        lines_with_prefix(&lines, "译文：").is_empty()
            && lines_with_prefix(&lines, "译文（原文回退）：").is_empty(),
        "原文视图不得渲染任何译文行：{lines:#?}"
    );
}

/// 译文视图：缺译段落回退原文并显式标注，不留空白。
#[test]
fn translation_view_falls_back_to_original_for_missing_gaps() {
    let mut session = session_with(MockContentApi::new(), MOCK_PARTIAL_CONTENT_KEY);
    session.load();
    session.set_mode(ViewMode::Translation);

    let lines = render_session(&session);
    assert_no_blank_lines(&lines);
    assert!(
        lines_with_prefix(&lines, "原文：").is_empty(),
        "译文视图不应显示原文列：{lines:#?}"
    );

    let translated = lines_with_prefix(&lines, "译文：");
    let fallbacks = lines_with_prefix(&lines, "译文（原文回退）：");
    assert_eq!(
        translated.len() + fallbacks.len(),
        5,
        "5 段都必须有一行译文（含回退），实际 translated={translated:#?} fallbacks={fallbacks:#?}"
    );
    assert_eq!(
        fallbacks.len(),
        2,
        "partial 章节有 2 段缺译文，必须回退原文"
    );
    assert!(
        fallbacks
            .iter()
            .any(|line| line.contains("A desktop reader built around blocks.")),
        "回退行必须显示该段原文，不能空白：{fallbacks:#?}"
    );
}

/// 对照视图下缺译文回退原文，并显式标注“原文回退”（不是空白、不前移）。
#[test]
fn compare_view_falls_back_to_original_for_missing_translation() {
    let mut session = session_with(MockContentApi::new(), MOCK_PARTIAL_CONTENT_KEY);
    session.load();

    let lines = render_session(&session);
    assert_no_blank_lines(&lines);
    let fallbacks = lines_with_prefix(&lines, "译文（原文回退）：");
    assert_eq!(
        fallbacks.len(),
        2,
        "partial 章节 2 段缺译文必须回退原文：{lines:#?}"
    );
    assert!(
        fallbacks
            .iter()
            .any(|line| line.contains("A desktop reader built around blocks.")),
        "回退行必须显示该段原文：{fallbacks:#?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("本段暂无译文")),
        "不应再出现每段占位句：{lines:#?}"
    );
}

/// 加载 / 空章节 / 失败三态都必须有明确提示，且不残留正文行。
#[test]
fn loading_empty_and_failed_states_render_explicit_hints() {
    // 加载中：还没有任何正文行。
    let session = session_with(MockContentApi::new(), MOCK_CONTENT_KEY);
    let loading = render_session(&session);
    assert_no_blank_lines(&loading);
    assert!(
        loading[0].contains("加载中"),
        "加载态页头必须说明正在加载，实际 {:?}",
        loading[0]
    );
    assert!(
        lines_with_prefix(&loading, "原文：").is_empty(),
        "加载态不得渲染正文行：{loading:#?}"
    );

    // 空章节：不是错误，要有明确提示。
    let mut empty = session_with(MockContentApi::new(), MOCK_EMPTY_CONTENT_KEY);
    empty.load();
    let empty_lines = render_session(&empty);
    assert_no_blank_lines(&empty_lines);
    assert!(
        empty_lines.iter().any(|line| line.contains(EMPTY_HINT)),
        "空章节必须提示 {EMPTY_HINT:?}，实际 {empty_lines:#?}"
    );
    assert!(
        lines_with_prefix(&empty_lines, "原文：").is_empty(),
        "空章节不得渲染正文行：{empty_lines:#?}"
    );

    // 失败：可展示原因 + 重试指引。
    let mut failed = session_with(MockContentApi::with_failures(1), MOCK_CONTENT_KEY);
    failed.load();
    let failed_lines = render_session(&failed);
    assert_no_blank_lines(&failed_lines);
    let joined = failed_lines.join("\n");
    assert!(
        joined.contains("请求失败") && joined.contains("按 r 重试"),
        "失败态必须给出原因与重试指引，实际 {failed_lines:#?}"
    );
    assert!(
        lines_with_prefix(&failed_lines, "原文：").is_empty(),
        "失败态不得残留正文行：{failed_lines:#?}"
    );
}

/// 长章节分页 + 视图切换：页码与阅读位置都不丢。
#[test]
fn paging_and_view_switching_preserve_reading_position() {
    let mut session = session_with(MockContentApi::new(), MOCK_LONG_CONTENT_KEY).with_page_size(20);
    session.load();
    session.set_page(2);

    for mode in ViewMode::ALL {
        session.set_mode(mode);
        let lines = render_session(&session);
        assert_no_blank_lines(&lines);
        assert!(
            lines[0].contains("第 3/6 页"),
            "视图 {} 下页码必须保持第 3/6 页，实际 {:?}",
            mode.label(),
            lines[0]
        );
        assert!(
            lines.iter().any(|line| line.starts_with("[41]")),
            "视图 {} 下第一段必须是第 41 段，实际 {lines:#?}",
            mode.label()
        );
        assert_eq!(session.anchor(), 40, "切换视图不得改变阅读位置");
    }

    assert!(session.prev_page(), "第 3 页可以回到第 2 页");
    assert_eq!(session.anchor(), 20, "翻页后阅读位置必须跟着走");
    let second_page = render_session(&session);
    assert!(
        second_page[0].contains("第 2/6 页") && second_page.iter().any(|l| l.starts_with("[21]")),
        "翻页后页头与段落都要更新，实际 {second_page:#?}"
    );
}
