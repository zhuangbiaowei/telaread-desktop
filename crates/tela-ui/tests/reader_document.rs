//! 切片 M3-UI-1：正文块 → 阅读段落的映射，以及译文「按块索引」逐段对齐。
//!
//! 契约来源：docs/development_plan.md §6 C11、§9 M3（块级渲染 + 三模式对照）；
//! docs/product-design.md §7.4（对照=左右双栏、块级对齐；缺翻译要有明确降级）。
//!
//! 断言真实数据：正文/译文全部来自 `tela_content` 的 mock API 与 fixture，
//! 组件侧不持有任何正文数据（步骤约束「组件只调用 API 层」）。

use tela_content::api::{ChapterRequest, ContentApi, mock_content_api};
use tela_content::mock::{MOCK_CONTENT_KEY, MOCK_PARTIAL_CONTENT_KEY};
use tela_ui::reader::{ReaderDocument, ViewMode};

/// 用默认装配（mock）取一章并映射成阅读文档。
fn load(key: &str) -> ReaderDocument {
    let api: Box<dyn ContentApi> = mock_content_api();
    let request = ChapterRequest::new(key, "zh-Hans");
    let chapter = api.load_chapter(&request).expect("mock 章节应可取到");
    ReaderDocument::from_chapter(&request, &chapter)
}

/// 段落顺序与正文块一一对应（不错位、不丢块），HTML 剥离为纯文本。
#[test]
fn paragraphs_follow_block_order_and_strip_markup() {
    let doc = load(MOCK_CONTENT_KEY);

    assert_eq!(doc.paragraphs.len(), 5, "5 个顶层块必须映射成 5 段");
    let tags: Vec<&str> = doc.paragraphs.iter().map(|p| p.tag.as_str()).collect();
    assert_eq!(
        tags,
        ["h1", "p", "figure", "pre", "blockquote"],
        "段落顺序必须与正文块顺序一致，不得错位"
    );

    assert_eq!(doc.paragraphs[0].original, "Hello Tela");
    assert_eq!(
        doc.paragraphs[1].original, "A desktop reader built around blocks.",
        "内联 <em> 标签要剥离成纯文本，不能丢掉里面的词"
    );
    assert_eq!(
        doc.paragraphs[2].original, "[图片：cover]",
        "图片块必须有非空降级文本，否则界面会出现空白行"
    );
    assert_eq!(
        doc.paragraphs[3].original, "fn main() { println!(\"hi\"); }",
        "代码块文本原样保留，不能被标签剥离吃掉内容"
    );
    assert_eq!(doc.paragraphs[4].original, "Reading is a conversation.");

    let indices: Vec<usize> = doc.paragraphs.iter().map(|p| p.index).collect();
    assert_eq!(indices, [0, 1, 2, 3, 4], "段落索引必须是连续块索引");
}

/// 完整译文按块索引对齐：第 N 段原文配第 N 段译文。
#[test]
fn complete_translation_aligns_paragraph_by_paragraph() {
    let doc = load(MOCK_CONTENT_KEY);

    assert_eq!(doc.total_paragraphs(), 5);
    assert_eq!(doc.gap_count(), 0, "完整译文不应有空洞");
    assert_eq!(doc.paragraphs[0].translation.as_deref(), Some("你好，Tela"));
    assert_eq!(
        doc.paragraphs[1].translation.as_deref(),
        Some("一个围绕块构建的桌面阅读器。"),
        "译文里的内联标签同样要剥离"
    );
    assert_eq!(doc.paragraphs[2].translation.as_deref(), Some("封面插图"));
    assert_eq!(
        doc.paragraphs[3].translation.as_deref(),
        Some("fn main() { println!(\"hi\"); }")
    );
    assert_eq!(
        doc.paragraphs[4].translation.as_deref(),
        Some("阅读是一场对话。")
    );
}

/// 部分译文：缺失段保留空洞、不前移，且对照视图给出明确降级文案（不是空白）。
#[test]
fn partial_translation_keeps_gaps_in_place_and_degrades_explicitly() {
    let doc = load(MOCK_PARTIAL_CONTENT_KEY);

    assert_eq!(doc.paragraphs.len(), 5, "正文段数不因译文缺失而变化");
    assert_eq!(doc.paragraphs[0].translation.as_deref(), Some("你好，Tela"));
    assert_eq!(
        doc.paragraphs[1].translation, None,
        "第 2 段译文缺失必须留空位，不能把第 3 段译文前移"
    );
    assert_eq!(doc.paragraphs[2].translation.as_deref(), Some("图：封面"));
    assert_eq!(
        doc.paragraphs[3].translation, None,
        "skip 叶子不翻译，对齐后仍是空洞"
    );
    assert_eq!(
        doc.paragraphs[4].translation.as_deref(),
        Some("阅读是一场对话。")
    );
    assert_eq!(doc.gap_count(), 2);

    let row = doc.row(1, ViewMode::Compare).expect("第 2 段存在");
    assert_eq!(
        row.translation, "（本段暂无译文）",
        "缺失译文在对照视图要有占位文案，禁止空白常驻"
    );
    assert!(row.translation_missing, "行上要能看出该段译文缺失");

    let present = doc.row(0, ViewMode::Compare).expect("第 1 段存在");
    assert_eq!(present.translation, "你好，Tela");
    assert!(!present.translation_missing);
}
