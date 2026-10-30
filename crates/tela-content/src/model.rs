//! 正文 / 译本 / 章节的数据模型，字段逐一对齐 OpenAPI `ContentObject` 与 `TranslationObject`。
//!
//! 关键约定（`docs/api/v1.md` § Bodies and images / § Translations）：
//!
//! - 正文对象**不可变**，由 `contentKey` 寻址，可以无限期缓存；
//! - `blocks[i]` 是已消毒的 HTML，`leaves` 是该块的 `data-tb` id 列表；
//! - 译本对象与正文**按块下标一一对应**，未翻译的块保留原文；
//! - `translation.object` / `translation.chunks` 是 `/o/…` 路径，不是裸 key。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use tela_proto::TelaError;

/// 正文对象（`GET /v1/bodies` 里的值，或 `/o/c/<key>.json`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentObject {
    /// 格式版本（当前恒为 1）。
    pub format: u32,
    /// 生成规则版本。
    #[serde(default)]
    pub norm: i64,
    /// 32 位小写十六进制 key。
    pub key: String,
    /// 正文语言。
    pub lang: String,
    /// 顶层块。
    #[serde(default)]
    pub blocks: Vec<ContentBlock>,
    /// 叶子 id → 元信息。
    #[serde(default)]
    pub leaves: BTreeMap<String, LeafInfo>,
    /// 原文图片 URL。
    #[serde(default)]
    pub images: Vec<String>,
    /// 统计。
    #[serde(default)]
    pub stats: ContentStats,
}

impl ContentObject {
    /// 空章节（`blocks: []`，语义是「正文为空」而不是错误）。
    pub fn empty(key: impl Into<String>) -> Self {
        Self {
            format: 1,
            norm: 0,
            key: key.into(),
            lang: "en".to_string(),
            blocks: Vec::new(),
            leaves: BTreeMap::new(),
            images: Vec::new(),
            stats: ContentStats::default(),
        }
    }

    /// 顶层块数。
    pub fn total_blocks(&self) -> usize {
        self.blocks.len()
    }

    /// 取一页块（长章节懒加载）。
    ///
    /// `limit == 0` 返回空页；`offset` 越界返回空页且 `has_more=false`，不 panic。
    pub fn page(&self, offset: usize, limit: usize) -> BlockPage<'_> {
        let total = self.blocks.len();
        if offset >= total {
            return BlockPage {
                blocks: &[],
                offset,
                total,
                has_more: false,
            };
        }
        let end = offset.saturating_add(limit).min(total);
        BlockPage {
            blocks: &self.blocks[offset..end],
            offset,
            total,
            has_more: end < total,
        }
    }
}

/// 顶层块。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentBlock {
    /// 语义标签（`h1` / `p` / `figure` / `pre` / `blockquote` / `table` …）。
    pub tag: String,
    /// 已消毒的 HTML。
    pub html: String,
    /// 该块内的叶子 id。
    #[serde(default)]
    pub leaves: Vec<String>,
}

/// 叶子元信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeafInfo {
    /// 内容 hash。
    #[serde(default)]
    pub hash: String,
    /// 字符数（服务端口径）。
    #[serde(default)]
    pub chars: i64,
    /// 服务端标记该叶子不翻译（代码块、过短文本、纯地址等）。
    #[serde(default)]
    pub skip: bool,
}

/// 正文统计。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentStats {
    /// 字符数。
    #[serde(default)]
    pub chars: i64,
    /// 词数。
    #[serde(default)]
    pub words: i64,
    /// 预计阅读分钟数。
    #[serde(default)]
    pub minutes: i64,
}

/// 一页顶层块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPage<'a> {
    /// 本页块切片。
    pub blocks: &'a [ContentBlock],
    /// 起始下标。
    pub offset: usize,
    /// 总块数。
    pub total: usize,
    /// 后续是否还有块。
    pub has_more: bool,
}

/// 译本对象（`/o/t/<key>/<lang>/<version>.json`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationObject {
    /// 格式版本。
    pub format: u32,
    /// 生成规则版本。
    #[serde(default)]
    pub norm: i64,
    /// 源正文 key。
    #[serde(default)]
    pub source: String,
    /// 译文语言。
    pub lang: String,
    /// 状态（open enum：`done` / `partial` / `failed` / …）。
    pub status: String,
    /// 与正文块**同序**的 HTML（未译块保留原文）。
    #[serde(default)]
    pub blocks: Vec<String>,
    /// 翻译失败的叶子 id。
    #[serde(default)]
    pub failed_leaves: Vec<String>,
}

impl TranslationObject {
    /// 按正文块数对齐；缺失或空串返回 `None`，长度恒等于 `content_blocks`。
    pub fn align(&self, content_blocks: usize) -> Vec<Option<String>> {
        (0..content_blocks)
            .map(|index| {
                self.blocks
                    .get(index)
                    .filter(|text| !text.is_empty())
                    .cloned()
            })
            .collect()
    }

    /// 是否部分翻译。
    pub fn is_partial(&self) -> bool {
        self.status == "partial" || !self.failed_leaves.is_empty()
    }
}

/// 译本行状态（open enum）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranslationState {
    /// 已请求，排队中。
    Requested,
    /// 翻译中。
    Running,
    /// 今日额度用完，等到 `dueAt`。
    Waiting,
    /// 完成。
    Done,
    /// 部分块保留原文。
    Partial,
    /// 失败。
    Failed,
    /// 跳过。
    Skipped,
    /// 未知状态：按文档当作不可用。
    #[serde(other)]
    Unknown,
}

impl TranslationState {
    /// 是否正在进行（可以轮询）。
    pub fn is_running(self) -> bool {
        matches!(self, Self::Requested | Self::Running)
    }

    /// 是否已有可读译本。
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Done | Self::Partial)
    }
}

/// `rows.translations` 的同形结构 / `GET /v1/translations/{key}/{lang}` 里的值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationRow {
    /// 正文 key。
    pub content_key: String,
    /// 目标语言。
    pub lang: String,
    /// 状态。
    pub state: TranslationState,
    /// 已完成分块路径。
    #[serde(default)]
    pub chunks: Vec<String>,
    /// 完成后的译本对象路径（`/o/t/…`）。
    #[serde(default)]
    pub object: Option<String>,
    /// `waiting` 的开始时间（epoch 毫秒）。
    #[serde(default)]
    pub due_at: Option<i64>,
}

/// `GET /v1/translations/{contentKey}/{lang}`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationStatusResponse {
    /// 没有译本时为 null。
    #[serde(default)]
    pub translation: Option<TranslationRow>,
}

/// `POST /v1/translations` 的 `status`（open enum）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranslationAnswerStatus {
    /// 已完成。
    Ready,
    /// 进行中。
    Running,
    /// 额度已满，等到 `translation.dueAt`。
    Waiting,
    /// 今天与明天都没有额度。
    Refused,
    /// 作者关闭翻译，或没有正文。
    Unavailable,
    /// 未知：按 unavailable 处理。
    #[serde(other)]
    Unknown,
}

/// `POST /v1/translations` 的响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationAnswer {
    /// 结果状态。
    pub status: TranslationAnswerStatus,
    /// 译本行（`unavailable` 时可能为 null）。
    #[serde(default)]
    pub translation: Option<TranslationRow>,
    /// `refused` 的原因：`member` 或 `site`。
    #[serde(default)]
    pub reason: Option<String>,
    /// `refused` 的重置时间。
    #[serde(default)]
    pub resets_at: Option<i64>,
}

/// UI 可直接消费的译文视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranslationView {
    /// 没有译文（未请求 / 作者关闭 / 跳过）。
    NotAvailable,
    /// 正在翻译或排队；界面显示原文并提示。
    Pending {
        /// 服务端状态。
        state: TranslationState,
        /// `waiting` 时的开始时间。
        due_at: Option<i64>,
    },
    /// 有译本对象；`aligned` 与正文块一一对应。
    Ready {
        /// 原始译本对象。
        object: TranslationObject,
        /// 对齐后的译文（`None` 表示该块没有译文）。
        aligned: Vec<Option<String>>,
    },
}

impl TranslationView {
    /// 由译本对象构造对齐视图。
    pub fn from_object(content_blocks: usize, object: TranslationObject) -> Self {
        let aligned = object.align(content_blocks);
        Self::Ready { object, aligned }
    }

    /// 对齐后的译文切片（仅 `Ready`）。
    pub fn aligned(&self) -> Option<&[Option<String>]> {
        match self {
            Self::Ready { aligned, .. } => Some(aligned),
            _ => None,
        }
    }

    /// 是否部分缺失。
    pub fn has_gaps(&self) -> bool {
        self.aligned()
            .is_some_and(|blocks| blocks.iter().any(Option::is_none))
    }
}

/// 一章：正文对象 + 译文视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    /// 正文。
    pub content: ContentObject,
    /// 译文。
    pub translation: TranslationView,
}

impl Chapter {
    /// 构造。
    pub fn new(content: ContentObject, translation: TranslationView) -> Self {
        Self {
            content,
            translation,
        }
    }

    /// 是否空章节。
    pub fn is_empty(&self) -> bool {
        self.content.blocks.is_empty()
    }

    /// 顶层块数。
    pub fn total_blocks(&self) -> usize {
        self.content.total_blocks()
    }
}

/// 章节加载四态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentState<T> {
    /// 加载中。
    Loading,
    /// 成功但内容为空。
    Empty,
    /// 成功且有内容。
    Ready(T),
    /// 失败。
    Failed(TelaError),
}

impl<T> ContentState<T> {
    /// 是否加载中。
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    /// 是否失败。
    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

impl ContentState<Chapter> {
    /// 把 API 结果映射成 UI 状态。
    pub fn from_result(result: Result<Chapter, TelaError>) -> Self {
        match result {
            Ok(chapter) if chapter.is_empty() => Self::Empty,
            Ok(chapter) => Self::Ready(chapter),
            Err(err) => Self::Failed(err),
        }
    }
}

/// `GET /v1/bodies` 的完整响应。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodiesResponse {
    /// 拿到的对象（key → 对象）。不存在的 key 既不在 `objects` 也不在 `refused`。
    #[serde(default)]
    pub objects: BTreeMap<String, ContentObject>,
    /// 因为每日非订阅文章额度被拒的 key。
    #[serde(default)]
    pub refused: BTreeMap<String, RefusedBody>,
}

/// 被每日文章额度拒绝的一篇。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefusedBody {
    /// 错误码（`post_limit` / `feed_limit`）。
    pub error: String,
    /// 上限。
    pub limit: i64,
    /// 重置时间（epoch 毫秒）。
    pub resets_at: i64,
}
