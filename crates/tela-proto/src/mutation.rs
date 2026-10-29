//! `POST /v1/mutations`：17 种变更、批量上限与结果（OpenAPI `PushRequest` / `Push`）。
//!
//! 三个硬约束：
//!
//! 1. 判别键是 `type`，其余字段 camelCase；
//! 2. 每条都带客户端自铸的 `mid`（8–40 字符，幂等重放）与客户端时间 `at`（epoch 毫秒，
//!    同一对象上「晚者胜」）；
//! 3. 可空字段里，**缺失与显式 null 是两种语义**（`recommend.note`、`putHighlight.lang`、
//!    `fileFeed.folderId`、`setReadingLanguage.language` 都必须能显式发 null），
//!    因此这些字段用 `Option<T>` 且不跳过序列化。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{MAX_MUTATIONS_PER_PUSH, MAX_PUSH_BYTES, TelaError, new_mid, now_ms};

/// 推荐语上限（字符）。
pub const MAX_RECOMMENDATION_NOTE: usize = 500;
/// 划线笔记上限（字符）。
pub const MAX_HIGHLIGHT_NOTE: usize = 2_000;
/// 划线选中文本上限（UTF-16 code unit）。
pub const MAX_HIGHLIGHT_QUOTE: usize = 2_000;

/// v1 允许的偏好键（`setPref` 只接受这些）。
pub const PREF_KEYS: [&str; 4] = [
    "reader.hide_read",
    "reader.mark_on_open",
    "translate.auto",
    "translate.never",
];

/// 所有 mutation 共有的字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationBase {
    /// 客户端自铸的 id，8–40 字符；重发同一个 mid 不会重复生效。
    pub mid: String,
    /// 客户端时间（epoch 毫秒）；超前于服务端时按服务端的 now 计。
    pub at: i64,
}

impl MutationBase {
    /// 用当前时间与新 mid 构造。
    pub fn now() -> Self {
        Self {
            mid: new_mid(),
            at: now_ms(),
        }
    }
}

/// 一次 push 里的单条变更。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Mutation {
    /// 标记已读。
    MarkRead {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 文章 id。
        article_id: i64,
    },
    /// 手工标记未读（直到下一次已读之前都保持未读）。
    MarkUnread {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 文章 id。
        article_id: i64,
    },
    /// 绝对设置点赞状态（不是 toggle）。
    SetLiked {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 文章 id。
        article_id: i64,
        /// 是否点赞。
        liked: bool,
    },
    /// 把 `upTo` 及之前的文章全部标为已读；`feedId` 缺失表示全部订阅。
    MarkAllRead {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 只对该 feed 生效。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feed_id: Option<i64>,
        /// 展示到的最高文章 id。
        up_to: i64,
    },
    /// 订阅一个 feed。
    Subscribe {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// feed id。
        feed_id: i64,
    },
    /// 退订一个 feed。
    Unsubscribe {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// feed id。
        feed_id: i64,
    },
    /// 写一条偏好。
    SetPref {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 偏好键（必须来自 [`PREF_KEYS`]）。
        key: String,
        /// 偏好值。
        value: Value,
    },
    /// 设置阅读语言；`language: null` 表示跟随界面语言。
    SetReadingLanguage {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 阅读语言，或 null。
        language: Option<String>,
    },
    /// 公开推荐一篇文章，可附推荐语（null 表示无推荐语）。
    Recommend {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 文章 id。
        article_id: i64,
        /// 推荐语，或 null。
        note: Option<String>,
    },
    /// 撤回推荐。
    Unrecommend {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 文章 id。
        article_id: i64,
    },
    /// 新建或编辑一条划线。
    PutHighlight {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 客户端自铸的划线 id。
        id: String,
        /// 文章 id。
        article_id: i64,
        /// 正文对象 key。
        content_key: String,
        /// 原文侧或译文侧。
        side: crate::HighlightSide,
        /// 译文侧语言；原文侧必须显式发 null。
        lang: Option<String>,
        /// 叶子 id（正文 HTML 的 `data-tb`）。
        leaf_id: String,
        /// 起点（UTF-16 code unit）。
        start: i64,
        /// 终点（UTF-16 code unit）。
        end: i64,
        /// 选中文本。
        quote: String,
        /// 前文（≤32 字符）。
        prefix: String,
        /// 后文（≤32 字符）。
        suffix: String,
        /// 笔记，或 null。
        note: Option<String>,
    },
    /// 删除一条划线（删除胜过之后的编辑）。
    DeleteHighlight {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 划线 id。
        id: String,
    },
    /// 关注一个成员。
    Follow {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 成员账号 id。
        user_id: String,
    },
    /// 取关一个成员。
    Unfollow {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 成员账号 id。
        user_id: String,
    },
    /// 新建或重命名文件夹。
    PutFolder {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 文件夹 id。
        id: String,
        /// 名称。
        name: String,
    },
    /// 删除文件夹（其中的博客会变成「不在文件夹」）。
    DeleteFolder {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// 文件夹 id。
        id: String,
    },
    /// 把 feed 放进文件夹；`folderId: null` 表示移出。
    FileFeed {
        /// 共有字段。
        #[serde(flatten)]
        base: MutationBase,
        /// feed id。
        feed_id: i64,
        /// 目标文件夹，或 null。
        folder_id: Option<String>,
    },
}

impl Mutation {
    /// 这条变更的 mid。
    pub fn mid(&self) -> &str {
        &self.base().mid
    }

    /// 这条变更的客户端时间。
    pub fn at(&self) -> i64 {
        self.base().at
    }

    /// 共有字段。
    pub fn base(&self) -> &MutationBase {
        match self {
            Self::MarkRead { base, .. }
            | Self::MarkUnread { base, .. }
            | Self::SetLiked { base, .. }
            | Self::MarkAllRead { base, .. }
            | Self::Subscribe { base, .. }
            | Self::Unsubscribe { base, .. }
            | Self::SetPref { base, .. }
            | Self::SetReadingLanguage { base, .. }
            | Self::Recommend { base, .. }
            | Self::Unrecommend { base, .. }
            | Self::PutHighlight { base, .. }
            | Self::DeleteHighlight { base, .. }
            | Self::Follow { base, .. }
            | Self::Unfollow { base, .. }
            | Self::PutFolder { base, .. }
            | Self::DeleteFolder { base, .. }
            | Self::FileFeed { base, .. } => base,
        }
    }

    /// wire 上的 `type` 值。
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::MarkRead { .. } => "markRead",
            Self::MarkUnread { .. } => "markUnread",
            Self::SetLiked { .. } => "setLiked",
            Self::MarkAllRead { .. } => "markAllRead",
            Self::Subscribe { .. } => "subscribe",
            Self::Unsubscribe { .. } => "unsubscribe",
            Self::SetPref { .. } => "setPref",
            Self::SetReadingLanguage { .. } => "setReadingLanguage",
            Self::Recommend { .. } => "recommend",
            Self::Unrecommend { .. } => "unrecommend",
            Self::PutHighlight { .. } => "putHighlight",
            Self::DeleteHighlight { .. } => "deleteHighlight",
            Self::Follow { .. } => "follow",
            Self::Unfollow { .. } => "unfollow",
            Self::PutFolder { .. } => "putFolder",
            Self::DeleteFolder { .. } => "deleteFolder",
            Self::FileFeed { .. } => "fileFeed",
        }
    }

    /// 用新的 mid 与 at 替换（离线队列重放时保持原值；这里给测试与迁移用）。
    pub fn restamp(mut self, mid: impl Into<String>, at: i64) -> Self {
        let base = match &mut self {
            Self::MarkRead { base, .. }
            | Self::MarkUnread { base, .. }
            | Self::SetLiked { base, .. }
            | Self::MarkAllRead { base, .. }
            | Self::Subscribe { base, .. }
            | Self::Unsubscribe { base, .. }
            | Self::SetPref { base, .. }
            | Self::SetReadingLanguage { base, .. }
            | Self::Recommend { base, .. }
            | Self::Unrecommend { base, .. }
            | Self::PutHighlight { base, .. }
            | Self::DeleteHighlight { base, .. }
            | Self::Follow { base, .. }
            | Self::Unfollow { base, .. }
            | Self::PutFolder { base, .. }
            | Self::DeleteFolder { base, .. }
            | Self::FileFeed { base, .. } => base,
        };
        base.mid = mid.into();
        base.at = at;
        self
    }

    // ---- 便捷构造（mid 与 at 自动填） ----

    /// 标记已读。
    pub fn mark_read(article_id: i64) -> Self {
        Self::MarkRead {
            base: MutationBase::now(),
            article_id,
        }
    }

    /// 标记未读。
    pub fn mark_unread(article_id: i64) -> Self {
        Self::MarkUnread {
            base: MutationBase::now(),
            article_id,
        }
    }

    /// 设置点赞。
    pub fn set_liked(article_id: i64, liked: bool) -> Self {
        Self::SetLiked {
            base: MutationBase::now(),
            article_id,
            liked,
        }
    }

    /// 全部标为已读。
    pub fn mark_all_read(feed_id: Option<i64>, up_to: i64) -> Self {
        Self::MarkAllRead {
            base: MutationBase::now(),
            feed_id,
            up_to,
        }
    }

    /// 订阅。
    pub fn subscribe(feed_id: i64) -> Self {
        Self::Subscribe {
            base: MutationBase::now(),
            feed_id,
        }
    }

    /// 退订。
    pub fn unsubscribe(feed_id: i64) -> Self {
        Self::Unsubscribe {
            base: MutationBase::now(),
            feed_id,
        }
    }

    /// 写偏好。
    pub fn set_pref(key: impl Into<String>, value: Value) -> Self {
        Self::SetPref {
            base: MutationBase::now(),
            key: key.into(),
            value,
        }
    }

    /// 设置阅读语言；`None` 表示跟随界面语言。
    pub fn set_reading_language(language: Option<String>) -> Self {
        Self::SetReadingLanguage {
            base: MutationBase::now(),
            language,
        }
    }

    /// 推荐。
    pub fn recommend(article_id: i64, note: Option<String>) -> Self {
        Self::Recommend {
            base: MutationBase::now(),
            article_id,
            note,
        }
    }

    /// 撤回推荐。
    pub fn unrecommend(article_id: i64) -> Self {
        Self::Unrecommend {
            base: MutationBase::now(),
            article_id,
        }
    }

    /// 关注。
    pub fn follow(user_id: impl Into<String>) -> Self {
        Self::Follow {
            base: MutationBase::now(),
            user_id: user_id.into(),
        }
    }

    /// 取关。
    pub fn unfollow(user_id: impl Into<String>) -> Self {
        Self::Unfollow {
            base: MutationBase::now(),
            user_id: user_id.into(),
        }
    }

    /// 新建/重命名文件夹。
    pub fn put_folder(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self::PutFolder {
            base: MutationBase::now(),
            id: id.into(),
            name: name.into(),
        }
    }

    /// 删除文件夹。
    pub fn delete_folder(id: impl Into<String>) -> Self {
        Self::DeleteFolder {
            base: MutationBase::now(),
            id: id.into(),
        }
    }

    /// 把 feed 放进文件夹（`None` 表示移出）。
    pub fn file_feed(feed_id: i64, folder_id: Option<String>) -> Self {
        Self::FileFeed {
            base: MutationBase::now(),
            feed_id,
            folder_id,
        }
    }
}

/// 一次 push 的请求体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushRequest {
    /// 本批变更，顺序即应用顺序。
    pub mutations: Vec<Mutation>,
}

/// 单条变更的结果状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MutationStatus {
    /// 已经生效（重放也算 applied）。
    Applied,
    /// 永远不会生效：丢弃并回滚界面上的乐观效果。
    Rejected,
    /// v1 之外的未知状态：按 rejected 处理。
    #[serde(other)]
    Unknown,
}

/// 一条变更的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationResult {
    /// 对应的 mid；没有可读 mid 时为 null（此时按位置匹配）。
    #[serde(default)]
    pub mid: Option<String>,
    /// 结果状态。
    pub status: MutationStatus,
    /// 被拒原因（open enum：`invalid` / `subscription_limit` / …）。
    #[serde(default)]
    pub error: Option<String>,
}

/// 一次 push 的响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResponse {
    /// 与请求**按位置**一一对应。
    #[serde(default)]
    pub results: Vec<MutationResult>,
}

impl PushResponse {
    /// 位置对应的结果；数量对不上时返回 `None`（视为协议违约）。
    pub fn result_at(&self, index: usize) -> Option<&MutationResult> {
        self.results.get(index)
    }

    /// 是否全部 applied。
    pub fn all_applied(&self) -> bool {
        self.results
            .iter()
            .all(|result| result.status == MutationStatus::Applied)
    }
}

/// 校验一批变更是否可以直接发出去。
///
/// 校验项与文档一致：条数 ≤ 50、序列化后 ≤ 1 MB、mid 8–40 字符、`at` 非负、
/// 划线的 `end > start` 且 `quote` 的 UTF-16 长度等于 `end - start`、
/// 推荐语/笔记长度上限、`setPref` 键在 allowlist 内。
pub fn validate_push(mutations: &[Mutation]) -> Result<(), TelaError> {
    if mutations.len() > MAX_MUTATIONS_PER_PUSH {
        return Err(TelaError::TooManyMutations {
            count: mutations.len(),
            max: MAX_MUTATIONS_PER_PUSH,
        });
    }

    for mutation in mutations {
        validate_mutation(mutation)?;
    }

    let body = serde_json::to_vec(&PushRequest {
        mutations: mutations.to_vec(),
    })
    .map_err(|err| TelaError::Config {
        detail: format!("变更无法序列化：{err}"),
    })?;
    if body.len() > MAX_PUSH_BYTES {
        return Err(TelaError::PushTooLarge {
            bytes: body.len(),
            max: MAX_PUSH_BYTES,
        });
    }
    Ok(())
}

fn validate_mutation(mutation: &Mutation) -> Result<(), TelaError> {
    let invalid = |detail: String| TelaError::InvalidMutation {
        mid: mutation.mid().to_string(),
        detail,
    };

    let mid_len = mutation.mid().chars().count();
    if !(8..=40).contains(&mid_len) {
        return Err(invalid(format!("mid 长度为 {mid_len}，要求 8–40 字符")));
    }
    if mutation.at() < 0 {
        return Err(invalid("at 不能为负（epoch 毫秒）".to_string()));
    }

    match mutation {
        Mutation::SetPref { key, .. } if !PREF_KEYS.contains(&key.as_str()) => {
            Err(invalid(format!(
                "偏好键 `{key}` 不在 v1 allowlist（{}）内",
                PREF_KEYS.join(" / ")
            )))
        }
        Mutation::Recommend { note, .. } => {
            if note
                .as_deref()
                .is_some_and(|note| note.chars().count() > MAX_RECOMMENDATION_NOTE)
            {
                return Err(invalid(format!(
                    "推荐语超过 {MAX_RECOMMENDATION_NOTE} 字符"
                )));
            }
            Ok(())
        }
        Mutation::PutHighlight {
            id,
            content_key,
            leaf_id,
            start,
            end,
            quote,
            prefix,
            suffix,
            note,
            ..
        } => {
            if !(8..=40).contains(&id.chars().count()) {
                return Err(invalid("划线 id 长度要求 8–40 字符".to_string()));
            }
            if content_key.len() != 32 || !content_key.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(invalid("contentKey 必须是 32 位十六进制".to_string()));
            }
            if leaf_id.is_empty() {
                return Err(invalid("leafId 不能为空".to_string()));
            }
            let quote_len = quote.encode_utf16().count();
            if *end <= *start {
                return Err(invalid("划线 end 必须大于 start".to_string()));
            }
            if i64::try_from(quote_len).ok() != Some(end - start) {
                return Err(invalid(format!(
                    "quote 的 UTF-16 长度 {quote_len} 不等于 end - start ({})",
                    end - start
                )));
            }
            if quote_len > MAX_HIGHLIGHT_QUOTE {
                return Err(invalid(format!(
                    "quote 超过 {MAX_HIGHLIGHT_QUOTE} 个 UTF-16 单位"
                )));
            }
            if prefix.chars().count() > 32 || suffix.chars().count() > 32 {
                return Err(invalid("prefix/suffix 各不得超过 32 字符".to_string()));
            }
            if note
                .as_deref()
                .is_some_and(|note| note.chars().count() > MAX_HIGHLIGHT_NOTE)
            {
                return Err(invalid(format!("笔记超过 {MAX_HIGHLIGHT_NOTE} 字符")));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_read_wire_shape_is_exact() {
        let mutation = Mutation::mark_read(42).restamp("m-00000001", 1_790_596_740_000);
        let value = serde_json::to_value(&mutation).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type": "markRead",
                "mid": "m-00000001",
                "at": 1_790_596_740_000_i64,
                "articleId": 42,
            })
        );
    }

    #[test]
    fn null_fields_are_serialized_not_skipped() {
        let mutation = Mutation::recommend(7, None).restamp("m-00000002", 1);
        let value = serde_json::to_value(&mutation).unwrap();
        assert_eq!(value["note"], Value::Null);

        let mutation = Mutation::file_feed(3, None).restamp("m-00000003", 1);
        assert_eq!(
            serde_json::to_value(&mutation).unwrap()["folderId"],
            Value::Null
        );

        let mutation = Mutation::set_reading_language(None).restamp("m-00000004", 1);
        assert_eq!(
            serde_json::to_value(&mutation).unwrap()["language"],
            Value::Null
        );
    }

    #[test]
    fn optional_mark_all_read_feed_is_omitted() {
        let mutation = Mutation::mark_all_read(None, 9).restamp("m-00000005", 1);
        let value = serde_json::to_value(&mutation).unwrap();
        assert!(value.get("feedId").is_none());
        assert_eq!(value["upTo"], 9);
    }

    #[test]
    fn all_seventeen_types_round_trip() {
        let mutations = vec![
            Mutation::mark_read(1),
            Mutation::mark_unread(2),
            Mutation::set_liked(3, true),
            Mutation::mark_all_read(Some(4), 5),
            Mutation::subscribe(6),
            Mutation::unsubscribe(7),
            Mutation::set_pref("reader.hide_read", Value::Bool(false)),
            Mutation::set_reading_language(Some("zh-Hans".into())),
            Mutation::recommend(8, Some("好".into())),
            Mutation::unrecommend(9),
            Mutation::PutHighlight {
                base: MutationBase::now(),
                id: "h-0000001".into(),
                article_id: 10,
                content_key: "c0ffee00000000000000000000000010".into(),
                side: crate::HighlightSide::Original,
                lang: None,
                leaf_id: "b1".into(),
                start: 0,
                end: 2,
                quote: "ab".into(),
                prefix: String::new(),
                suffix: String::new(),
                note: None,
            },
            Mutation::DeleteHighlight {
                base: MutationBase::now(),
                id: "h-0000001".into(),
            },
            Mutation::follow("u-1"),
            Mutation::unfollow("u-2"),
            Mutation::put_folder("f-0000001", "Reading"),
            Mutation::delete_folder("f-0000002"),
            Mutation::file_feed(11, Some("f-0000001".into())),
        ];
        assert_eq!(mutations.len(), 17);
        validate_push(&mutations).unwrap();
        let json = serde_json::to_string(&PushRequest { mutations }).unwrap();
        let back: PushRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mutations.len(), 17);
    }
}
