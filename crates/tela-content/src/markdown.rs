//! Markdown → 阅读器块：收藏网页以 Markdown 为规范格式，阅读面板渲染前先转回 HTML 块。
//!
//! 为什么不用 Markdown 专用渲染器：桌面阅读面板已经能渲染经过清洗的块 HTML
//! （标题/段落/引用/列表/代码/表格/图片，见 `crate::webpage`）。把 Markdown 用
//! `pulldown-cmark` 转成 HTML，再复用同一套清洗/分块规则，三件事同时成立：
//!
//! - 存储层只存一份 Markdown（可导出、可给 LLM 用）；
//! - 阅读面板不需要第二套渲染语义；
//! - 图片地址与网页收藏一致，仍然走 `cached_images` 缓存。
//!
//! 安全边界：`pulldown-cmark` 会原样输出 Markdown 里的裸 HTML，但随后经过
//! [`crate::webpage::blocks_from_html`] 的白名单清洗，脚本/事件属性/危险链接都会被丢掉。

use std::collections::BTreeMap;

use pulldown_cmark::{Options, Parser, html};
use tela_proto::TelaError;

use crate::model::{ContentBlock, ContentObject, ContentStats};
use crate::webpage::{blocks_from_html, reading_minutes, stats_from_blocks};

/// Markdown → HTML 片段（`pulldown-cmark`，开启表格/删除线/任务列表/脚注）。
pub fn to_html(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut html = String::with_capacity(markdown.len());
    html::push_html(&mut html, Parser::new_ext(markdown, options));
    html
}

/// Markdown → 阅读器顶层块（与 [`ContentObject::blocks`] 同形）。
pub fn to_blocks(markdown: &str) -> Vec<ContentBlock> {
    let (blocks, _images) = blocks_from_html(&to_html(markdown), None);
    blocks
}

/// Markdown → 阅读器正文对象。
///
/// 图片地址保持 Markdown 里的绝对地址：阅读面板会用 `ContentApi::load_image`
/// 从本地图片缓存读取，离线也能显示。
pub fn to_content(
    key: impl Into<String>,
    lang: impl Into<String>,
    markdown: &str,
) -> Result<ContentObject, TelaError> {
    let (blocks, images) = blocks_from_html(&to_html(markdown), None);
    let (chars, words, _excerpt) = stats_from_blocks(&blocks);
    Ok(ContentObject {
        format: 1,
        norm: 0,
        key: key.into(),
        lang: lang.into(),
        blocks,
        leaves: BTreeMap::new(),
        images,
        stats: ContentStats {
            chars,
            words,
            minutes: reading_minutes(words),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html_of(markdown: &str) -> String {
        to_html(markdown)
    }

    #[test]
    fn headings_paragraphs_and_lists_become_blocks() {
        let markdown = "# 标题\n\n第一段。\n\n- 一\n- 二\n\n> 引用\n\n```rust\nfn main() {}\n```\n";
        let blocks = to_blocks(markdown);
        let tags: Vec<&str> = blocks.iter().map(|block| block.tag.as_str()).collect();
        assert_eq!(tags, vec!["h1", "p", "ul", "blockquote", "pre"]);
        assert!(blocks[4].html.contains("fn main()"));
        assert!(blocks[2].html.contains("一"));
        assert!(blocks[2].html.contains("二"));
    }

    #[test]
    fn tables_links_and_images_are_kept() {
        let markdown = "| 名称 | 值 |\n| --- | --- |\n| a | 1 |\n\n[链接](https://example.com/a)\n\n![图](https://example.com/a.png)\n";
        let blocks = to_blocks(markdown);
        let html: String = blocks.iter().map(|b| b.html.as_str()).collect();
        assert!(html.contains('a'), "{html}");
        assert!(html.contains("https://example.com/a"), "{html}");
        assert!(html.contains("https://example.com/a.png"), "{html}");
    }

    #[test]
    fn images_are_collected_for_offline_caching() {
        let content = to_content(
            "web-test",
            "zh-Hans",
            "![封面](https://example.com/cover.png)\n\n正文。\n",
        )
        .unwrap();
        assert_eq!(
            content.images,
            vec!["https://example.com/cover.png".to_string()]
        );
        assert!(content.stats.chars > 0);
    }

    #[test]
    fn raw_html_in_markdown_is_sanitized() {
        let markdown = "<script>alert(1)</script>\n\n<p onclick=\"steal()\">文字</p>\n";
        let blocks = to_blocks(markdown);
        let html: String = blocks.iter().map(|b| b.html.as_str()).collect();
        assert!(!html.contains("script"), "{html}");
        assert!(!html.contains("onclick"), "{html}");
        assert!(html.contains("文字"), "{html}");
    }

    #[test]
    fn markdown_to_html_renders_common_blocks() {
        let html = html_of("# 标题\n\n一段\n");
        assert!(html.contains("<h1>标题</h1>"));
        assert!(html.contains("<p>一段</p>"));
    }
}
