//! 正文块 → 阅读段落映射（切片 M3-UI-1）。
//!
//! 契约来源：docs/development_plan.md §6 C11、§9 M3；docs/product-design.md §7.4。
//!
//! 这一层只做纯映射：把 `tela-content` 的 `Chapter`（顶层块 + 已对齐译文）
//! 变成界面可直接消费的段落序列。规则：
//!
//! 1. **一段 = 一个顶层块**，索引连续且稳定（不错位、不合并、不丢块）；
//! 2. 原文/译文都从 HTML 片段剥离为纯文本，图片等无文字块给非空降级文本，
//!    否则界面会出现常驻空白行；
//! 3. 译文按块索引对齐，缺失段保持 `None` 空洞（不前移），渲染时回退原文；
//!    代码块、图片块、过短文本等被服务端标记为 `skip` 的段落不算“缺译文”。

use tela_content::{Chapter, ChapterRequest, TranslationView};

use crate::reader::ViewMode;

/// 图片块在无可用图片渲染时的降级文本前缀。
pub const IMAGE_PLACEHOLDER: &str = "[图片";

/// 一个可渲染的阅读段落（= 一个正文顶层块）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderParagraph {
    /// 块索引（连续、稳定，用于对齐与阅读位置）。
    pub index: usize,
    /// 语义标签（`h1` / `p` / `figure` / `pre` / `blockquote` …）。
    pub tag: String,
    /// 该块的叶子 id（稳定锚点，后续高亮/定位用）。
    pub leaves: Vec<String>,
    /// 原始 HTML（GUI 富文本渲染用：标题、链接、代码、图片、引用）。
    pub original_html: String,
    /// 剥离标签后的原文。
    pub original: String,
    /// 对齐后的译文 HTML；`None` = 该段没有译文（保持空洞，不前移）。
    pub translation_html: Option<String>,
    /// 对齐后的译文；`None` = 该段没有译文（保持空洞，不前移）。
    pub translation: Option<String>,
    /// 该段是否有需要翻译的文字。
    ///
    /// 代码块、纯图片块、过短文本、纯地址等由服务端标记为不翻译；它们没有
    /// 译文是正常的，不应计入“译文部分缺失”。
    pub translatable: bool,
}

impl ReaderParagraph {
    /// 该段是否缺译文。
    pub fn translation_missing(&self) -> bool {
        self.translation.is_none()
    }
}

/// 一行的渲染数据：原文列 + 译文列（对照视图）/ 单列（原文 / 译文视图）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderRow {
    /// 块索引。
    pub index: usize,
    /// 语义标签。
    pub tag: String,
    /// 原文 HTML（GUI 富文本渲染）。
    pub original_html: String,
    /// 原文列内容。
    pub original: String,
    /// 译文 HTML（GUI 富文本渲染）；缺译文时为 `None`。
    pub translation_html: Option<String>,
    /// 译文列内容；缺译文时为占位文案或原文回退，绝不为空白。
    pub translation: String,
    /// 该段译文是否缺失（界面据此显示降级样式）。
    pub translation_missing: bool,
    /// 当前视图是否要显示原文列。
    pub show_original: bool,
    /// 当前视图是否要显示译文列。
    pub show_translation: bool,
}

/// 译文的整体可用状态（界面用它在顶部给一句说明）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslationStatus {
    /// 没有译文对象（未请求 / 404 / 全部跳过）。
    Missing,
    /// 服务端仍在翻译。
    Pending,
    /// 有译文且无空洞。
    Complete,
    /// 有译文但有空洞（部分段落没翻）。
    Partial,
}

impl TranslationStatus {
    /// 顶部的状态说明文案。
    pub fn label(self) -> &'static str {
        match self {
            Self::Missing => "本章暂无译文，已显示原文",
            Self::Pending => "翻译进行中，先显示原文",
            Self::Complete => "译文已就绪",
            Self::Partial => "译文部分缺失（未翻译段落显示原文）",
        }
    }

    /// 是否完全没有译文可用。
    pub fn is_missing(self) -> bool {
        matches!(self, Self::Missing | Self::Pending)
    }
}

/// 一章的阅读文档：段落序列 + 译文状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderDocument {
    /// 正文对象 key（后续取图/定位用）。
    pub content_key: String,
    /// 目标译文语言。
    pub lang: String,
    /// 原文语言（来自正文对象）。
    pub source_lang: String,
    /// 译文整体状态。
    pub translation_status: TranslationStatus,
    /// 按块顺序排列的段落。
    pub paragraphs: Vec<ReaderParagraph>,
    /// 正文（含译文）里出现的图片 URL，按首次出现顺序去重。
    ///
    /// GUI 用它在后台带认证下载图片；这里只收集，不做 IO。
    pub image_sources: Vec<String>,
}

impl ReaderDocument {
    /// 由 API 层的 `Chapter` 构造阅读文档（纯映射，无 IO）。
    pub fn from_chapter(request: &ChapterRequest, chapter: &Chapter) -> Self {
        let aligned = chapter.translation.aligned();

        let mut image_sources = Vec::new();
        let paragraphs: Vec<ReaderParagraph> = chapter
            .content
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| {
                let raw_translation = aligned
                    .and_then(|list| list.get(index).cloned().flatten())
                    .filter(|raw| !raw.trim().is_empty());
                let translation = raw_translation
                    .as_deref()
                    .map(html_to_text)
                    .filter(|text| !text.is_empty());
                let translation_html = if translation.is_some() {
                    raw_translation
                } else {
                    None
                };

                // 服务端会把代码块、纯图、过短文本、纯地址的叶子标成 skip；
                // 这类块没有译文是设计如此，不参与“缺译”统计。
                let translatable = block.tag != "pre"
                    && !block.leaves.is_empty()
                    && block.leaves.iter().any(|id| {
                        chapter
                            .content
                            .leaves
                            .get(id)
                            .map(|info| !info.skip)
                            .unwrap_or(true)
                    });

                push_images(&mut image_sources, &block.html);
                if let Some(html) = &translation_html {
                    push_images(&mut image_sources, html);
                }

                ReaderParagraph {
                    index,
                    tag: block.tag.clone(),
                    leaves: block.leaves.clone(),
                    original_html: block.html.clone(),
                    original: html_to_text(&block.html),
                    translation_html,
                    translation,
                    translatable,
                }
            })
            .collect();

        let translatable_count = paragraphs.iter().filter(|p| p.translatable).count();
        let gaps = paragraphs
            .iter()
            .filter(|p| p.translatable && p.translation_missing())
            .count();
        let translation_status = match &chapter.translation {
            TranslationView::NotAvailable => TranslationStatus::Missing,
            TranslationView::Pending { .. } => TranslationStatus::Pending,
            TranslationView::Ready { .. } => {
                if aligned.is_none() || (translatable_count > 0 && gaps == translatable_count) {
                    TranslationStatus::Missing
                } else if gaps > 0 {
                    TranslationStatus::Partial
                } else {
                    TranslationStatus::Complete
                }
            }
        };

        Self {
            content_key: request.content_key.clone(),
            lang: request.lang.clone(),
            source_lang: chapter.content.lang.clone(),
            translation_status,
            paragraphs,
            image_sources,
        }
    }

    /// 段落总数。
    pub fn total_paragraphs(&self) -> usize {
        self.paragraphs.len()
    }

    /// 空章节（没有任何顶层块）。
    pub fn is_empty(&self) -> bool {
        self.paragraphs.is_empty()
    }

    /// 缺译文的段落数（只统计本来就需要翻译的段落）。
    pub fn gap_count(&self) -> usize {
        self.paragraphs
            .iter()
            .filter(|p| p.translatable && p.translation_missing())
            .count()
    }

    /// 需要翻译的段落数。
    pub fn translatable_count(&self) -> usize {
        self.paragraphs.iter().filter(|p| p.translatable).count()
    }

    /// 是否有任何译文可用。
    pub fn has_translation(&self) -> bool {
        let translatable = self.translatable_count();
        !self.translation_status.is_missing()
            && (translatable == 0 || self.gap_count() < translatable)
    }

    /// 取某段在当前视图下的渲染行；索引越界返回 `None`。
    pub fn row(&self, index: usize, mode: ViewMode) -> Option<ReaderRow> {
        let paragraph = self.paragraphs.get(index)?;
        // 缺译文时回退原文（与 Web 一致）：代码/图片/超出翻译额度的段落
        // 仍然可读，不再在每段中间插一句“本段暂无译文”。
        let (translation, translation_missing) = match &paragraph.translation {
            Some(text) => (text.clone(), false),
            None => (paragraph.original.clone(), true),
        };

        Some(ReaderRow {
            index: paragraph.index,
            tag: paragraph.tag.clone(),
            original_html: paragraph.original_html.clone(),
            original: paragraph.original.clone(),
            translation_html: paragraph.translation_html.clone(),
            translation,
            translation_missing,
            show_original: !matches!(mode, ViewMode::Translation),
            show_translation: !matches!(mode, ViewMode::Original),
        })
    }

    /// 取一页渲染行（终端分页 / 长文按需渲染用），`offset` 越界时返回空 `Vec`。
    pub fn rows(&self, mode: ViewMode, offset: usize, limit: usize) -> Vec<ReaderRow> {
        self.paragraphs
            .iter()
            .skip(offset)
            .take(limit)
            .filter_map(|p| self.row(p.index, mode))
            .collect()
    }

    /// 取整篇渲染行（桌面端连续滚动阅读用，不分页）。
    ///
    /// 段落顺序、索引与 [`Self::rows`] 完全一致；一次给全，界面不需要
    /// 再维护“当前第几页”，用户一直按 PgDn / ↓ 就能读完。
    pub fn all_rows(&self, mode: ViewMode) -> Vec<ReaderRow> {
        self.paragraphs
            .iter()
            .filter_map(|p| self.row(p.index, mode))
            .collect()
    }
}

/// 收集 HTML 片段里的图片 URL（按出现顺序，不处理 `data:` 内联图）。
///
/// 服务端已把正文图片改写成 `/img/<key>/<i>`，阅读面板需要先知道有哪些
/// 地址，才能在后台带认证下载。`data:` 图由 GPUI 自己解码，不走这里。
pub fn image_sources_in_html(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;

    while let Some(lt) = rest.find('<') {
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..gt];
        if tag_name(tag) == "img"
            && let Some(src) = attribute(tag, "src")
        {
            let src = decode_entities(src.trim());
            if !src.is_empty() && !src.starts_with("data:") && !out.iter().any(|item| item == &src)
            {
                out.push(src);
            }
        }
        rest = &rest[gt + 1..];
    }

    out
}

/// 追加图片地址并去重（原文与译文经常引用同一张图）。
fn push_images(out: &mut Vec<String>, html: &str) {
    for src in image_sources_in_html(html) {
        if !out.iter().any(|item| item == &src) {
            out.push(src);
        }
    }
}

/// HTML 片段 → 纯文本（剥离标签、解码常见实体、归一化空白）。
///
/// 无文字的块（图片等）必须有非空降级文本：`<img alt="cover">` → `[图片：cover]`，
/// 否则阅读视图会留下空白行。
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else {
            // 未闭合的 `<`：按纯文本处理，避免吞掉正文。
            break;
        };
        let tag = &rest[1..gt];
        let name = tag_name(tag);
        if name == "img" {
            let alt = attribute(tag, "alt").unwrap_or_default();
            let alt = alt.trim();
            if alt.is_empty() {
                out.push_str(IMAGE_PLACEHOLDER);
                out.push(']');
            } else {
                out.push_str(IMAGE_PLACEHOLDER);
                out.push('：');
                out.push_str(alt);
                out.push(']');
            }
        }
        rest = &rest[gt + 1..];
    }
    out.push_str(rest);

    normalize_whitespace(&decode_entities(&out))
}

/// 标签名（小写），`</p>` 也能正确取到 `p`。
fn tag_name(tag: &str) -> String {
    tag.trim_start()
        .trim_start_matches('/')
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// 取标签属性值（大小写不敏感，支持单/双引号与裸值）。
fn attribute(tag: &str, name: &str) -> Option<String> {
    let chars: Vec<char> = tag.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '=' {
            i += 1;
        }
        let key: String = chars[start..i].iter().collect();

        let mut value = String::new();
        if i < chars.len() && chars[i] == '=' {
            i += 1;
            if i < chars.len() && (chars[i] == '"' || chars[i] == '\'') {
                let quote = chars[i];
                i += 1;
                while i < chars.len() && chars[i] != quote {
                    value.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() {
                    i += 1;
                }
            } else {
                while i < chars.len() && !chars[i].is_whitespace() {
                    value.push(chars[i]);
                    i += 1;
                }
            }
        }

        if key.eq_ignore_ascii_case(name) {
            return Some(value);
        }
        if start == i {
            // 防御：无进展时前进一格，避免死循环。
            i += 1;
        }
    }

    None
}

/// 解码常见 HTML 实体（正文/译文里的 `&amp;` `&lt;` `&nbsp;` 等）。
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|dec| dec.parse::<u32>().ok())
                })
                .and_then(char::from_u32),
        };

        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &rest[semi + 1..];
            }
            None => {
                // 不是实体：保留 `&` 继续往后扫。
                out.push('&');
                rest = &rest[1..];
            }
        }
    }

    out.push_str(rest);
    out
}

/// 空白归一化：水平空白折叠为一个空格、去除空行，保留换行以维持代码块层级。
fn normalize_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    let mut last_was_newline = false;

    for ch in text.chars() {
        match ch {
            '\n' => {
                // 行尾空格丢弃，连续空行折叠为一个换行。
                while out.ends_with(' ') {
                    out.pop();
                }
                if !out.is_empty() && !last_was_newline {
                    out.push('\n');
                }
                last_was_newline = true;
                pending_space = false;
            }
            c if c.is_whitespace() => {
                pending_space = true;
            }
            c => {
                if pending_space && !out.is_empty() && !last_was_newline {
                    out.push(' ');
                }
                pending_space = false;
                last_was_newline = false;
                out.push(c);
            }
        }
    }

    while out.ends_with(' ') || out.ends_with('\n') {
        out.pop();
    }
    out
}

/// 取出 `<pre>` 里的代码文本（保留空白、换行与实体），供 GUI 自定义代码块渲染。
///
/// GPUI 的 HTML 视图把 `<pre><code>` 里的 `<code>` 当作行内代码处理，会给
/// 每一行套上浅色圆角背景；阅读面板因此单独渲染 `pre` 块，保持等宽、深色底。
pub fn preformatted_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let inner = match lower.find("<pre") {
        Some(start) => {
            let after_open = lower[start..]
                .find('>')
                .map(|offset| start + offset + 1)
                .unwrap_or(start + 4);
            let end = lower
                .rfind("</pre>")
                .filter(|end| *end >= after_open)
                .unwrap_or(html.len());
            &html[after_open..end]
        }
        None => html,
    };
    let text = strip_tags_preserving_whitespace(inner);
    text.strip_prefix('\n')
        .unwrap_or(&text)
        .trim_end_matches('\n')
        .to_string()
}

/// 剥离标签但保留换行与缩进（代码块专用；`<br>` 转成换行）。
fn strip_tags_preserving_whitespace(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..gt];
        let name = tag_name(tag);
        match name.as_str() {
            "br" => out.push('\n'),
            "img" => {
                if let Some(alt) = attribute(tag, "alt").filter(|alt| !alt.trim().is_empty()) {
                    out.push_str(IMAGE_PLACEHOLDER);
                    out.push('：');
                    out.push_str(alt.trim());
                    out.push(']');
                }
            }
            _ => {}
        }
        rest = &rest[gt + 1..];
    }
    out.push_str(rest);
    decode_entities(&out)
}
