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
//! 3. 译文按块索引对齐，缺失段保持 `None` 空洞（不前移），渲染时给明确占位文案。

use tela_content::{Chapter, ChapterRequest, TranslationView};

use crate::reader::ViewMode;

/// 译文缺失时对照视图的占位文案（禁止空白常驻）。
pub const MISSING_TRANSLATION: &str = "（本段暂无译文）";

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
    /// 剥离标签后的原文。
    pub original: String,
    /// 对齐后的译文；`None` = 该段没有译文（保持空洞，不前移）。
    pub translation: Option<String>,
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
    /// 原文列内容。
    pub original: String,
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
            Self::Partial => "译文部分缺失，缺失段落已标注",
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
}

impl ReaderDocument {
    /// 由 API 层的 `Chapter` 构造阅读文档（纯映射，无 IO）。
    pub fn from_chapter(request: &ChapterRequest, chapter: &Chapter) -> Self {
        let aligned = chapter.translation.aligned();
        let translation_status = match &chapter.translation {
            TranslationView::NotAvailable => TranslationStatus::Missing,
            TranslationView::Pending { .. } => TranslationStatus::Pending,
            TranslationView::Ready { .. } => {
                let gaps = aligned
                    .map(|list| list.iter().filter(|slot| slot.is_none()).count())
                    .unwrap_or(chapter.content.blocks.len());
                if aligned.is_none() || gaps == chapter.content.blocks.len() {
                    TranslationStatus::Missing
                } else if gaps > 0 {
                    TranslationStatus::Partial
                } else {
                    TranslationStatus::Complete
                }
            }
        };

        let paragraphs = chapter
            .content
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| {
                let translation = aligned
                    .and_then(|list| list.get(index).cloned().flatten())
                    .map(|raw| html_to_text(&raw))
                    .filter(|text| !text.is_empty());
                ReaderParagraph {
                    index,
                    tag: block.tag.clone(),
                    leaves: block.leaves.clone(),
                    original: html_to_text(&block.html),
                    translation,
                }
            })
            .collect();

        Self {
            content_key: request.content_key.clone(),
            lang: request.lang.clone(),
            source_lang: chapter.content.lang.clone(),
            translation_status,
            paragraphs,
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

    /// 缺译文的段落数（对照视图的降级计数）。
    pub fn gap_count(&self) -> usize {
        self.paragraphs
            .iter()
            .filter(|p| p.translation_missing())
            .count()
    }

    /// 是否有任何译文可用。
    pub fn has_translation(&self) -> bool {
        !self.translation_status.is_missing() && self.gap_count() < self.paragraphs.len()
    }

    /// 取某段在当前视图下的渲染行；索引越界返回 `None`。
    pub fn row(&self, index: usize, mode: ViewMode) -> Option<ReaderRow> {
        let paragraph = self.paragraphs.get(index)?;
        let (translation, translation_missing) = match &paragraph.translation {
            Some(text) => (text.clone(), false),
            None => (
                match mode {
                    // 译文视图缺译文时回退原文，而不是留空白。
                    ViewMode::Translation => paragraph.original.clone(),
                    // 原文 / 对照视图给出显式占位文案。
                    ViewMode::Original | ViewMode::Compare => MISSING_TRANSLATION.to_string(),
                },
                true,
            ),
        };

        Some(ReaderRow {
            index: paragraph.index,
            tag: paragraph.tag.clone(),
            original: paragraph.original.clone(),
            translation,
            translation_missing,
            show_original: !matches!(mode, ViewMode::Translation),
            show_translation: !matches!(mode, ViewMode::Original),
        })
    }

    /// 取一页渲染行（分页/懒加载用），`offset` 越界时返回空 `Vec`。
    pub fn rows(&self, mode: ViewMode, offset: usize, limit: usize) -> Vec<ReaderRow> {
        self.paragraphs
            .iter()
            .skip(offset)
            .take(limit)
            .filter_map(|p| self.row(p.index, mode))
            .collect()
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
