//! 切片 M3-UI-1：正文块 → 阅读段落的映射，以及译文「按块索引」逐段对齐。
//!
//! 契约来源：docs/development_plan.md §6 C11、§9 M3（块级渲染 + 三模式对照）；
//! docs/product-design.md §7.4（对照=左右双栏、块级对齐；缺翻译要有明确降级）。
//!
//! 断言真实数据：正文/译文全部来自 `tela_content` 的 mock API 与 fixture，
//! 组件侧不持有任何正文数据（步骤约束「组件只调用 API 层」）。

use tela_content::api::{ChapterRequest, ContentApi, mock_content_api};
use tela_content::mock::{MOCK_CONTENT_KEY, MOCK_PARTIAL_CONTENT_KEY};
use tela_ui::reader::document::TranslationStatus;
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
    assert_eq!(
        doc.gap_count(),
        1,
        "只有需要翻译的段落算缺口；代码块被服务端 skip，不算缺译"
    );
    assert!(!doc.paragraphs[3].translatable, "skip 的代码块不应要求译文");
    assert!(!doc.paragraphs[2].translatable, "纯图片块没有可翻译文字");

    let row = doc.row(1, ViewMode::Compare).expect("第 2 段存在");
    assert_eq!(
        row.translation, "A desktop reader built around blocks.",
        "缺失译文回退原文，不再插入“本段暂无译文”"
    );
    assert!(row.translation_missing, "行上仍能看出该段译文缺失");

    let present = doc.row(0, ViewMode::Compare).expect("第 1 段存在");
    assert_eq!(present.translation, "你好，Tela");
    assert!(!present.translation_missing);
}

/// GUI 富文本渲染需要原始 HTML，同时图片地址要在读正文时收集好，
/// 这样阅读面板才能带认证下载，而不是把 `/img/...` 直接交给 GPUI。
#[test]
fn rich_html_and_image_sources_are_preserved_for_gui() {
    let doc = load(MOCK_CONTENT_KEY);

    assert!(
        doc.paragraphs[1].original_html.contains("<em>blocks</em>"),
        "原文 HTML 必须保留内联标记，GUI 才能按富文本渲染"
    );
    assert_eq!(
        doc.paragraphs[2].original_html,
        "<figure><img src=\"https://cdn.example.com/cover.png\" alt=\"cover\"></figure>"
    );
    assert_eq!(
        doc.image_sources,
        vec!["https://cdn.example.com/cover.png".to_string()],
        "图片地址应去重后按出现顺序收集"
    );

    assert_eq!(
        doc.paragraphs[1].translation_html.as_deref(),
        Some("一个围绕<em>块</em>构建的桌面阅读器。"),
        "译文 HTML 同样保留，供译文栏富文本渲染"
    );

    let row = doc.row(2, ViewMode::Compare).expect("图片段存在");
    assert_eq!(row.original_html, doc.paragraphs[2].original_html);
    assert_eq!(
        row.translation_html.as_deref(),
        Some("封面插图"),
        "图片段的译文没有 HTML 标签时也要能渲染"
    );
}

/// 原文与译文引用同一张图时，只下载一次。
#[test]
fn duplicate_image_sources_are_deduplicated() {
    let html =
        "<p><img src=\"/img/aa/0\"></p><p><img src='/img/aa/0'></p><p><img src=\"/img/aa/1\"></p>";
    let sources = tela_ui::reader::document::image_sources_in_html(html);
    assert_eq!(
        sources,
        vec!["/img/aa/0".to_string(), "/img/aa/1".to_string()]
    );
}

/// 代码块保留缩进、换行与实体，GUI 才能正常显示，不被 `<code>` 行内样式破坏。
#[test]
fn preformatted_text_keeps_code_layout() {
    let html = "<pre data-tb=\"b3\"><code class=\"language-rust\">fn main() {\n    println!(\"hi\");\n}</code></pre>";
    let code = tela_ui::reader::document::preformatted_text(html);
    assert_eq!(code, "fn main() {\n    println!(\"hi\");\n}");
    assert!(!code.contains('<'), "代码文本不能再带标签");
}

/// 代码块/纯图片块没有译文是正常情况：整篇状态仍是 Complete，不能报“部分缺失”。
#[test]
fn skipped_blocks_do_not_make_a_translation_partial() {
    use std::collections::BTreeMap;

    use tela_content::ChapterRequest;
    use tela_content::model::{
        Chapter, ContentBlock, ContentObject, ContentStats, LeafInfo, TranslationObject,
        TranslationView,
    };

    let content = ContentObject {
        format: 1,
        norm: 1,
        key: "skipped-only".to_string(),
        lang: "en".to_string(),
        blocks: vec![ContentBlock {
            tag: "pre".to_string(),
            html: "<pre><code>let x = 1;</code></pre>".to_string(),
            leaves: vec!["c1".to_string()],
        }],
        leaves: BTreeMap::from([(
            "c1".to_string(),
            LeafInfo {
                hash: "h-c1".to_string(),
                chars: 9,
                skip: true,
            },
        )]),
        images: Vec::new(),
        stats: ContentStats::default(),
    };
    let translation = TranslationView::from_object(
        1,
        TranslationObject {
            format: 1,
            norm: 1,
            source: "skipped-only".to_string(),
            lang: "zh-Hans".to_string(),
            status: "done".to_string(),
            blocks: vec![String::new()],
            failed_leaves: Vec::new(),
        },
    );
    let reader = ReaderDocument::from_chapter(
        &ChapterRequest::new("skipped-only", "zh-Hans"),
        &Chapter::new(content, translation),
    );

    assert!(!reader.paragraphs[0].translatable);
    assert_eq!(reader.gap_count(), 0, "skip 块不应计入缺口");
    assert_eq!(reader.translation_status, TranslationStatus::Complete);
}
