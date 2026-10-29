//! 离线 mock 内容 API：数据来自 `tests/fixtures/content/`，不内嵌在 UI 组件里。
//!
//! 覆盖场景（与 fixture 一一对应）：
//!
//! | 常量 | 场景 |
//! |---|---|
//! | [`MOCK_CONTENT_KEY`] | 正常章节（5 块）+ 完整译文 |
//! | [`MOCK_EMPTY_CONTENT_KEY`] | 空章节（0 块） |
//! | [`MOCK_LONG_CONTENT_KEY`] | 超长章节（120 块，分页/懒加载） |
//! | [`MOCK_PARTIAL_CONTENT_KEY`] | 译文部分缺失（对齐空洞 + failedLeaves） |
//! | [`MOCK_NO_TRANSLATION_CONTENT_KEY`] | 完全无译文 |
//!
//! 请求失败用 [`MockContentApi::fail_next`] 一次性注入，便于 UI 验证错误态。

use std::sync::atomic::{AtomicUsize, Ordering};

use serde::de::DeserializeOwned;
use tela_proto::{TelaError, WireContext};

use crate::api::{ChapterRequest, ContentApi};
use crate::model::{
    Chapter, ContentBlock, ContentObject, ContentStats, LeafInfo, TranslationAnswer,
    TranslationAnswerStatus, TranslationObject, TranslationRow, TranslationState, TranslationView,
};

/// 正常章节 key（对应 `tests/fixtures/content/content-normal.json`）。
pub const MOCK_CONTENT_KEY: &str = "11111111111111111111111111111111";
/// 空章节 key。
pub const MOCK_EMPTY_CONTENT_KEY: &str = "22222222222222222222222222222222";
/// 超长章节 key。
pub const MOCK_LONG_CONTENT_KEY: &str = "33333333333333333333333333333333";
/// 部分译文 key。
pub const MOCK_PARTIAL_CONTENT_KEY: &str = "44444444444444444444444444444444";
/// 无译文 key。
pub const MOCK_NO_TRANSLATION_CONTENT_KEY: &str = "55555555555555555555555555555555";

/// 超长章节的顶层块数量（分页/懒加载场景，固定值便于断言）。
pub const MOCK_LONG_CHAPTER_BLOCKS: usize = 120;

/// 把 `--chapter` 里可读的场景别名解析成稳定 key（README/`--help` 声明的 `mock-*` 参数）。
///
/// 未知取值原样返回，由 [`content_object`] 报「没有该 fixture」，因此别名解析不会掩盖错误。
pub fn resolve_content_key(raw: &str) -> &str {
    match raw {
        "mock" | "mock-normal" | "normal" => MOCK_CONTENT_KEY,
        "mock-empty" | "empty" => MOCK_EMPTY_CONTENT_KEY,
        "mock-long" | "long" => MOCK_LONG_CONTENT_KEY,
        "mock-partial" | "partial" => MOCK_PARTIAL_CONTENT_KEY,
        "mock-no-translation" | "no-translation" | "untranslated" => {
            MOCK_NO_TRANSLATION_CONTENT_KEY
        }
        other => other,
    }
}

/// mock 正常章节的正文 fixture（`include_str!` 保证离线、无文件 IO）。
const NORMAL_CONTENT_JSON: &str =
    include_str!("../../../tests/fixtures/content/content-normal.json");
/// mock 正常章节的完整中文译文 fixture。
const NORMAL_TRANSLATION_ZH_HANS_JSON: &str =
    include_str!("../../../tests/fixtures/content/translation-normal.zh-Hans.json");
/// mock 空章节的正文 fixture（`blocks: []`）。
const EMPTY_CONTENT_JSON: &str = include_str!("../../../tests/fixtures/content/content-empty.json");

/// 生成超长章节正文对象：块数固定、叶子 id 稳定，可复现。
///
/// 内容由代码生成而不是手写 fixture（120 块手写不可维护），但仍留在 mock 层，
/// UI 组件不持有任何正文数据。
fn long_chapter_object() -> ContentObject {
    let mut leaves = std::collections::BTreeMap::new();
    let blocks: Vec<ContentBlock> = (0..MOCK_LONG_CHAPTER_BLOCKS)
        .map(|index| {
            let leaf = format!("lb{}", index + 1);
            let tag = if index % 20 == 0 { "h2" } else { "p" };
            let text = if tag == "h2" {
                format!("Section {} of a long chapter", index / 20 + 1)
            } else {
                format!("Paragraph {} of a long chapter used for paging.", index + 1)
            };
            leaves.insert(
                leaf.clone(),
                LeafInfo {
                    hash: format!("h-{leaf}"),
                    chars: text.chars().count() as i64,
                },
            );
            ContentBlock {
                tag: tag.to_string(),
                html: format!("<{tag} data-tb=\"{leaf}\">{text}</{tag}>"),
                leaves: vec![leaf],
            }
        })
        .collect();

    ContentObject {
        format: 1,
        norm: 1,
        key: MOCK_LONG_CONTENT_KEY.to_string(),
        lang: "en".to_string(),
        blocks,
        leaves,
        images: Vec::new(),
        stats: ContentStats {
            chars: 0,
            words: 0,
            minutes: 0,
        },
    }
}

/// 生成「整章没有译本」的正文对象：3 个块，覆盖原文照常显示、译文整体缺失的场景。
fn untranslated_chapter_object() -> ContentObject {
    let mut leaves = std::collections::BTreeMap::new();
    let blocks: Vec<ContentBlock> = [
        ("h1", "n1", "A chapter nobody translated yet"),
        (
            "p",
            "n2",
            "The reader must fall back to the original language.",
        ),
        (
            "p",
            "n3",
            "TranslationView::NotAvailable keeps the text readable.",
        ),
    ]
    .into_iter()
    .map(|(tag, leaf, text)| {
        leaves.insert(
            leaf.to_string(),
            LeafInfo {
                hash: format!("h-{leaf}"),
                chars: text.chars().count() as i64,
            },
        );
        ContentBlock {
            tag: tag.to_string(),
            html: format!("<{tag} data-tb=\"{leaf}\">{text}</{tag}>"),
            leaves: vec![leaf.to_string()],
        }
    })
    .collect();

    ContentObject {
        format: 1,
        norm: 1,
        key: MOCK_NO_TRANSLATION_CONTENT_KEY.to_string(),
        lang: "en".to_string(),
        blocks,
        leaves,
        images: Vec::new(),
        stats: ContentStats::default(),
    }
}

/// 按 key 组装正文对象：正常/空章节来自 fixture，超长与无译文章节由 builder 生成。
fn content_object(content_key: &str) -> Result<ContentObject, TelaError> {
    match content_key {
        MOCK_CONTENT_KEY => parse_fixture(NORMAL_CONTENT_JSON, content_key, "正文"),
        MOCK_EMPTY_CONTENT_KEY => parse_fixture(EMPTY_CONTENT_JSON, content_key, "正文"),
        MOCK_LONG_CONTENT_KEY => Ok(long_chapter_object()),
        MOCK_NO_TRANSLATION_CONTENT_KEY => Ok(untranslated_chapter_object()),
        MOCK_PARTIAL_CONTENT_KEY => partial_content_object(),
        _ => Err(TelaError::Content {
            article: 0,
            content_key: content_key.to_string(),
            detail: "mock 没有该正文 key 的 fixture；可用 key 见 tela_content::mock 常量"
                .to_string(),
        }),
    }
}

/// 部分译文章节的正文：与正常章节同构（5 块），只把 key 换成 [`MOCK_PARTIAL_CONTENT_KEY`]，
/// 这样「同一篇正文、译文只翻了一半」的对照场景与正常章节可逐块对比。
fn partial_content_object() -> Result<ContentObject, TelaError> {
    let mut object: ContentObject =
        parse_fixture(NORMAL_CONTENT_JSON, MOCK_PARTIAL_CONTENT_KEY, "正文")?;
    object.key = MOCK_PARTIAL_CONTENT_KEY.to_string();
    Ok(object)
}

/// 部分译本：`status=partial` + `failedLeaves`，空洞块用空串占位。
///
/// 第 2 块（叶子 `b2`）与第 4 块（代码块 `b3`）保留失败标记，对齐后都是 `None`；
/// 其余块按位置对齐，不会前移。
fn partial_translation_object() -> TranslationObject {
    TranslationObject {
        format: 1,
        norm: 1,
        source: MOCK_PARTIAL_CONTENT_KEY.to_string(),
        lang: "zh-Hans".to_string(),
        status: "partial".to_string(),
        blocks: vec![
            "你好，Tela".to_string(),
            String::new(),
            "图：封面".to_string(),
            String::new(),
            "阅读是一场对话。".to_string(),
        ],
        failed_leaves: vec!["b2".to_string(), "b3".to_string()],
    }
}

/// 按 `(key, lang)` 返回译本对象；`None` 表示该语言没有译本（不是错误）。
fn translation_object(
    content_key: &str,
    lang: &str,
) -> Result<Option<TranslationObject>, TelaError> {
    match (content_key, lang) {
        (MOCK_CONTENT_KEY, "zh-Hans") => Ok(Some(parse_fixture(
            NORMAL_TRANSLATION_ZH_HANS_JSON,
            content_key,
            "译文",
        )?)),
        (MOCK_PARTIAL_CONTENT_KEY, "zh-Hans") => Ok(Some(partial_translation_object())),
        _ => Ok(None),
    }
}

/// 解析 fixture；失败时给出三段式 `TelaError::Content`。
fn parse_fixture<T: DeserializeOwned>(
    raw: &str,
    content_key: &str,
    kind: &str,
) -> Result<T, TelaError> {
    serde_json::from_str(raw).map_err(|err| TelaError::Content {
        article: 0,
        content_key: content_key.to_string(),
        detail: format!("mock {kind} fixture 解析失败：{err}"),
    })
}

/// 进程内 mock 内容 API。
#[derive(Debug, Default)]
pub struct MockContentApi {
    /// 还需注入的失败次数（一次性，生效后自减）。
    fail_next: AtomicUsize,
}

impl MockContentApi {
    /// 构造不注入失败的 mock。
    pub fn new() -> Self {
        Self::default()
    }

    /// 构造一个接下来 `times` 次请求都失败的 mock。
    pub fn with_failures(times: usize) -> Self {
        let api = Self::new();
        api.fail_next(times);
        api
    }

    /// 让接下来 `times` 次 `load_chapter` 返回 `TelaError::Net`。
    pub fn fail_next(&self, times: usize) {
        self.fail_next.store(times, Ordering::SeqCst);
    }

    /// 尚未消费的失败注入次数。
    pub fn remaining_failures(&self) -> usize {
        self.fail_next.load(Ordering::SeqCst)
    }

    /// 消费一次失败注入；返回 `true` 表示本次请求应失败。
    fn consume_injected_failure(&self) -> bool {
        self.fail_next
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
    }
}

impl ContentApi for MockContentApi {
    fn load_chapter(&self, request: &ChapterRequest) -> Result<Chapter, TelaError> {
        // 失败注入优先于取数：模拟「服务端不可达」，一次性消费，便于 UI 验证错误态。
        if self.consume_injected_failure() {
            return Err(injected_failure(&request.content_key));
        }

        // `--chapter mock-long` 之类的可读别名先解析成稳定 key，否则文档声明的常用参数
        // 会落到「没有该 fixture」的失败页（启动方式与项目声明不一致）。
        let content_key = resolve_content_key(&request.content_key);

        let content: ContentObject = content_object(content_key)?;

        let translation = match translation_object(content_key, &request.lang)? {
            Some(object) => TranslationView::from_object(content.total_blocks(), object),
            None => TranslationView::NotAvailable,
        };

        Ok(Chapter::new(content, translation))
    }

    fn request_translation(
        &self,
        article_id: i64,
        lang: &str,
    ) -> Result<TranslationAnswer, TelaError> {
        let _ = article_id;
        let content_key = resolve_content_key(MOCK_CONTENT_KEY).to_string();
        let answer = match translation_object(&content_key, lang)? {
            Some(_) => TranslationAnswer {
                status: TranslationAnswerStatus::Ready,
                translation: Some(mock_row(&content_key, lang, TranslationState::Done)),
                reason: None,
                resets_at: None,
            },
            None => TranslationAnswer {
                status: TranslationAnswerStatus::Unavailable,
                translation: None,
                reason: None,
                resets_at: None,
            },
        };
        Ok(answer)
    }

    fn translation_status(
        &self,
        content_key: &str,
        lang: &str,
    ) -> Result<Option<TranslationRow>, TelaError> {
        Ok(translation_object(content_key, lang)?
            .map(|_| mock_row(content_key, lang, TranslationState::Done)))
    }
}

/// mock 的译本行。
fn mock_row(content_key: &str, lang: &str, state: TranslationState) -> TranslationRow {
    TranslationRow {
        content_key: content_key.to_string(),
        lang: lang.to_string(),
        state,
        chunks: Vec::new(),
        object: Some(format!("/o/t/{content_key}/{lang}/mock.json")),
        due_at: None,
    }
}

/// 失败注入用的错误（mock 内部）。
pub(crate) fn injected_failure(content_key: &str) -> TelaError {
    TelaError::Net {
        ctx: WireContext::new(format!("mock://content/{content_key}"), "content"),
        detail: "mock injected failure：模拟服务端不可达（fail_next）".to_string(),
    }
}
