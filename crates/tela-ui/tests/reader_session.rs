//! 切片 M3-UI-2：阅读会话四态（加载 / 就绪 / 空章节 / 失败重试）。
//!
//! 契约来源：docs/development_plan.md §6 C11、§9 M3；docs/product-design.md §7.4。
//!
//! 断言真实状态迁移与渲染行内容：正文与译文全部来自 `tela_content` 的 mock API，
//! UI 层不持有任何正文数据（组件只调用 API 层）。

use std::sync::Arc;

use tela_content::api::ChapterRequest;
use tela_content::mock::{
    MOCK_CONTENT_KEY, MOCK_EMPTY_CONTENT_KEY, MOCK_LONG_CONTENT_KEY, MockContentApi,
};
use tela_ui::reader::ViewMode;
use tela_ui::reader::session::{ReaderSession, ReaderState};

/// 用默认页大小构造会话（mock 后端）。
fn session_with(api: MockContentApi, key: &str) -> ReaderSession {
    ReaderSession::new(Arc::new(api), ChapterRequest::new(key, "zh-Hans"))
}

/// 用指定页大小构造会话（长章节分页用）。
fn paged_session_with(api: MockContentApi, key: &str, page_size: usize) -> ReaderSession {
    session_with(api, key).with_page_size(page_size)
}

/// 取一页行索引序列（断言段落不错位、不重复）。
fn row_indices(session: &ReaderSession) -> Vec<usize> {
    session.visible_rows().iter().map(|row| row.index).collect()
}

/// 初始状态是加载态，取数后进入就绪态并带出按段落对照的行。
#[test]
fn starts_loading_then_ready_with_aligned_rows() {
    let mut session = session_with(MockContentApi::new(), MOCK_CONTENT_KEY);

    assert!(
        session.state().is_loading(),
        "会话初始必须是 Loading，实际 {:?}",
        session.state()
    );
    assert!(
        session.visible_rows().is_empty(),
        "加载中不应有可渲染的行（避免常驻空白占位）"
    );

    session.load();

    assert!(
        session.state().is_ready(),
        "mock 取数成功后应为 Ready，实际 {:?}",
        session.state()
    );
    assert_eq!(session.mode(), ViewMode::Compare, "默认视图为对照");

    let rows = session.visible_rows();
    assert_eq!(rows.len(), 5, "正常章节 5 段应全部可见（页大小 20）");
    assert_eq!(rows[0].original, "Hello Tela");
    assert_eq!(
        rows[0].translation, "你好，Tela",
        "第 1 段译文必须与第 1 段原文同排对齐"
    );
    assert_eq!(rows[1].original, "A desktop reader built around blocks.");
    assert_eq!(rows[1].translation, "一个围绕块构建的桌面阅读器。");
    assert!(
        rows[0].show_original && rows[0].show_translation,
        "对照视图两列都显示"
    );
}

/// 空章节是「请求成功但没有内容」：Empty 态、不是错误、无行、提示非空。
#[test]
fn empty_chapter_is_empty_state_not_error() {
    let mut session = session_with(MockContentApi::new(), MOCK_EMPTY_CONTENT_KEY);

    session.load();

    assert_eq!(
        *session.state(),
        ReaderState::Empty,
        "空章节必须是 Empty，不能落成 Failed"
    );
    assert!(
        session.state().error_message().is_none(),
        "空章节不显示错误文案"
    );
    assert!(session.visible_rows().is_empty(), "空章节没有任何渲染行");
    assert!(
        !session.state().status_label().is_empty(),
        "空章节必须有明确提示文案（不能让界面空白常驻）"
    );
}

/// 请求失败给出可展示的错误文案；重试成功后进入就绪态并清空错误。
#[test]
fn failed_load_exposes_error_and_retry_recovers() {
    let api = MockContentApi::with_failures(1);
    let mut session = session_with(api, MOCK_CONTENT_KEY);

    session.load();

    assert!(session.state().is_failed(), "注入失败后应为 Failed");
    let message = session
        .state()
        .error_message()
        .expect("失败态必须带可展示的原因");
    assert!(
        message.contains("请求失败"),
        "错误文案应说明是请求失败，实际 {message:?}"
    );
    assert!(session.visible_rows().is_empty(), "失败态不残留旧行");

    session.retry();

    assert!(
        session.state().is_ready(),
        "重试（失败注入已用完）后应恢复 Ready，实际 {:?}",
        session.state()
    );
    assert!(
        session.state().error_message().is_none(),
        "恢复后不再显示错误"
    );
    assert_eq!(session.visible_rows().len(), 5, "恢复后正文完整可见");
}

/// 长章节按页切片：每段恰好出现一次、页间无重叠无缺口、末页不越界。
#[test]
fn long_chapter_pages_cover_every_paragraph_exactly_once() {
    let mut session = paged_session_with(MockContentApi::new(), MOCK_LONG_CONTENT_KEY, 20);
    session.load();

    let total = session.total_paragraphs();
    assert_eq!(total, 120, "mock 超长章节应为 120 段，实际 {total}");
    assert_eq!(session.page_count(), 6, "120 段 / 每页 20 段 = 6 页");
    assert_eq!(session.page(), 0, "加载后停留在第 1 页");
    assert_eq!(
        row_indices(&session),
        (0..20).collect::<Vec<_>>(),
        "第 1 页必须是连续的第 1~20 段"
    );

    let mut seen = row_indices(&session);
    while session.next_page() {
        seen.extend(row_indices(&session));
    }

    assert_eq!(
        seen,
        (0..120).collect::<Vec<_>>(),
        "翻完所有页必须正好覆盖 0..120：不重复、不缺口、不错位"
    );
    assert_eq!(session.page(), 5, "末页是第 6 页");
    assert!(!session.next_page(), "末页再翻页应返回 false（不越界）");
    assert_eq!(session.anchor(), 100, "末页锚点停在最后一段页首");
}

/// 跳页与按段落定位：越界自动夹到合法范围，锚点始终是页首。
#[test]
fn page_navigation_clamps_at_bounds_and_keeps_anchor_on_page_start() {
    let mut session = paged_session_with(MockContentApi::new(), MOCK_LONG_CONTENT_KEY, 20);
    session.load();

    assert!(!session.prev_page(), "首页再上翻应返回 false");
    assert_eq!(session.anchor(), 0);

    assert!(session.next_page(), "首页可以下翻");
    assert_eq!(session.anchor(), 20, "第 2 页锚点是第 21 段（索引 20）");

    session.set_page(2);
    assert_eq!(session.anchor(), 40, "跳到第 3 页锚点为索引 40");

    session.set_page(99);
    assert_eq!(session.page(), 5, "越界跳页夹到末页");
    assert_eq!(session.anchor(), 100, "末页锚点为索引 100");

    session.goto_paragraph(45);
    assert_eq!(session.page(), 2, "定位到第 46 段应落在第 3 页");
    assert_eq!(session.anchor(), 40, "定位后锚点回到该页页首");
    assert_eq!(row_indices(&session).first().copied(), Some(40));
}
