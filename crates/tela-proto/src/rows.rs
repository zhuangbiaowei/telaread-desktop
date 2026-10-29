//! 同步行表（`SyncRows`）与各行类型。
//!
//! 逐字段对齐 `docs/api/openapi.json` 的 `Sync.rows`（2026-10-28）。三个容易踩的点：
//!
//! 1. **时间一律 epoch 毫秒**（旧 mock 协议是秒）；
//! 2. **行不再有 `seq`**：游标（`cursor`）是不透明字符串，由服务端决定粒度；
//! 3. **未知字段一律忽略**：v1 只做加法，客户端必须读得过去不认识的字段/表。
//!
//! `rows.profile` 是**一个对象或 null**，不是数组；其余 13 张是数组。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一次 pull 返回的全部行表。
///
/// `#[serde(default)]` 让缺失的表解成空值。未知表键由 serde 默认忽略——这是 v1
/// 「未知字段忽略」的直接要求，不能再像旧协议那样把未知表当协议违约。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRows {
    /// 成员资料：一个对象，或 `null`（本页没变）。
    #[serde(default)]
    pub profile: Option<ProfileRow>,
    /// 偏好项（只含 v1 allowlist 里的键）。
    #[serde(default)]
    pub prefs: Vec<PrefRow>,
    /// 成员订阅的 feed；`deletedAt` 非空表示退订。
    #[serde(default)]
    pub subscriptions: Vec<SubscriptionRow>,
    /// feed 元信息（**不等于**成员订阅：还含被收藏文章/博客卷里的博客）。
    #[serde(default)]
    pub feeds: Vec<FeedRow>,
    /// 博客（站点）元信息。
    #[serde(default)]
    pub sites: Vec<SiteRow>,
    /// 文章元信息。
    #[serde(default)]
    pub articles: Vec<ArticleRow>,
    /// 标题/摘要译文，键为 `(articleId, lang)`。
    #[serde(default)]
    pub titles: Vec<TitleRow>,
    /// 阅读/点赞状态，键为 `articleId`。
    #[serde(default)]
    pub states: Vec<StateRow>,
    /// 成员的公开推荐，键为 `articleId`。
    #[serde(default)]
    pub recommendations: Vec<RecommendationRow>,
    /// 私有划线，键为 `id`。
    #[serde(default)]
    pub highlights: Vec<HighlightRow>,
    /// 正文译本行，键为 `(contentKey, lang)`。
    #[serde(default)]
    pub translations: Vec<TranslationRow>,
    /// 私有文件夹，键为 `id`。
    #[serde(default)]
    pub folders: Vec<FolderRow>,
    /// feed 落在哪个文件夹，键为 `feedId`。
    #[serde(default)]
    pub feed_folders: Vec<FeedFolderRow>,
    /// 关注的人，键为 `userId`。
    #[serde(default)]
    pub follows: Vec<FollowRow>,
}

impl SyncRows {
    /// v1 默认的 14 张表名，顺序与文档一致。
    pub const TABLE_NAMES: [&'static str; 14] = crate::DEFAULT_SYNC_TYPES;

    /// 本页携带的行总数（不含 profile）。
    pub fn row_count(&self) -> usize {
        self.prefs.len()
            + self.subscriptions.len()
            + self.feeds.len()
            + self.sites.len()
            + self.articles.len()
            + self.titles.len()
            + self.states.len()
            + self.recommendations.len()
            + self.highlights.len()
            + self.translations.len()
            + self.folders.len()
            + self.feed_folders.len()
            + self.follows.len()
            + usize::from(self.profile.is_some())
    }
}

impl ProfileRow {
    /// 行的稳定主键：资料表只有一个成员，固定为 `profile`。
    pub const ROW_KEY: &'static str = "profile";
}

/// `rows.profile`：成员资料。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRow {
    /// 唯一 handle。
    pub handle: String,
    /// 显示名；可空。
    #[serde(default)]
    pub display_name: Option<String>,
    /// 简介；可空。
    #[serde(default)]
    pub bio: Option<String>,
    /// 头像绝对 URL；可空。
    #[serde(default)]
    pub avatar: Option<String>,
    /// 阅读语言；`null` 表示跟随界面语言。
    #[serde(default)]
    pub reading_language: Option<String>,
    /// 是否公开订阅列表。
    #[serde(default)]
    pub public_subscriptions: bool,
    /// 是否公开点赞。
    #[serde(default)]
    pub public_likes: bool,
}

/// `rows.prefs`：一条偏好。`value` 的类型由 key 决定，未知 key 忽略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefRow {
    /// 偏好键（例如 `reader.hide_read`）。
    pub key: String,
    /// 偏好值（布尔或字符串数组等）。
    pub value: Value,
    /// 最近更新时间（epoch 毫秒）。
    pub updated_at: i64,
}

/// `rows.subscriptions`：一条订阅关系。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionRow {
    /// 被订阅的 feed id。
    pub feed_id: i64,
    /// 已读水位线：文章 id ≤ 该值即已读。
    pub watermark_id: i64,
    /// 订阅时间（epoch 毫秒）。
    pub created_at: i64,
    /// 退订时间；非空即退订。
    #[serde(default)]
    pub deleted_at: Option<i64>,
}

/// `rows.feeds`：feed 元信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedRow {
    /// feed id。
    pub id: i64,
    /// 所属站点 id。
    pub site_id: i64,
    /// 抓取地址。
    pub feed_url: String,
    /// 标题；可空。
    #[serde(default)]
    pub title: Option<String>,
    /// 状态（open enum：`active` / `paused` / `dead` / …）。
    pub status: String,
    /// 最近抓取时间（epoch 毫秒）。
    #[serde(default)]
    pub last_fetched_at: Option<i64>,
    /// 最近一条文章的发布时间。
    #[serde(default)]
    pub last_item_at: Option<i64>,
}

/// `rows.sites`：博客（站点）元信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteRow {
    /// 站点 id。
    pub id: i64,
    /// 首页地址。
    pub home_url: String,
    /// 标题；可空。
    #[serde(default)]
    pub title: Option<String>,
    /// 简介；可空。
    #[serde(default)]
    pub description: Option<String>,
    /// 图标绝对 URL；可空。
    #[serde(default)]
    pub favicon_url: Option<String>,
    /// 主语言；可空。
    #[serde(default)]
    pub primary_lang: Option<String>,
    /// 读者数（三人以下服务端给 0）。
    #[serde(default)]
    pub reader_count: i64,
    /// 作者是否允许翻译其文章。
    #[serde(default = "default_true")]
    pub translation_allowed: bool,
    /// 是否已被认领。
    #[serde(default)]
    pub claimed: bool,
}

fn default_true() -> bool {
    true
}

/// `rows.articles`：文章元信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArticleRow {
    /// 文章 id。
    pub id: i64,
    /// 所属 feed id。
    pub feed_id: i64,
    /// 原文地址；可空。
    #[serde(default)]
    pub url: Option<String>,
    /// 标题（服务端保证非空）。
    pub title: String,
    /// 作者；可空。
    #[serde(default)]
    pub author: Option<String>,
    /// 发布时间（epoch 毫秒）。
    #[serde(default)]
    pub published_at: Option<i64>,
    /// 首次抓取时间：地平线从这里起算。
    pub fetched_at: i64,
    /// 列表排序时间。
    pub sort_at: i64,
    /// 原文语言；可空。
    #[serde(default)]
    pub source_lang: Option<String>,
    /// 摘要；可空。
    #[serde(default)]
    pub excerpt: Option<String>,
    /// 正文对象 key（32 位小写十六进制）；Tela 没有正文时为 null。
    #[serde(default)]
    pub content_key: Option<String>,
    /// 字数。
    #[serde(default)]
    pub word_count: i64,
    /// 预计阅读分钟数。
    #[serde(default)]
    pub reading_minutes: i64,
    /// 点赞数。
    #[serde(default)]
    pub like_count: i64,
    /// 推荐数。
    #[serde(default)]
    pub recommend_count: i64,
}

/// `rows.titles`：某文章某语言的标题/摘要译文。只发存在的译文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleRow {
    /// 文章 id。
    pub article_id: i64,
    /// 译文语言。
    pub lang: String,
    /// 译文标题；可空。
    #[serde(default)]
    pub title: Option<String>,
    /// 译文摘要；可空。
    #[serde(default)]
    pub excerpt: Option<String>,
}

/// `rows.states`：阅读/点赞状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateRow {
    /// 文章 id。
    pub article_id: i64,
    /// 已读时间；非空即已读。
    #[serde(default)]
    pub read_at: Option<i64>,
    /// 手工标为未读且此后未再读：优先于水位线与地平线。
    #[serde(default)]
    pub marked_unread: bool,
    /// 点赞时间；非空即已赞。
    #[serde(default)]
    pub liked_at: Option<i64>,
}

/// `rows.recommendations`：成员的公开推荐。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationRow {
    /// 文章 id。
    pub article_id: i64,
    /// 推荐语；可空。
    #[serde(default)]
    pub note: Option<String>,
    /// 推荐时间（epoch 毫秒）。
    pub created_at: i64,
    /// 撤回时间；非空即已撤回。
    #[serde(default)]
    pub deleted_at: Option<i64>,
}

/// 划线锚定在哪一侧。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HighlightSide {
    /// 原文。
    Original,
    /// 译文。
    Translation,
}

impl HighlightSide {
    /// wire 文本。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Translation => "translation",
        }
    }
}

/// `rows.highlights`：一条私有划线（含笔记）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HighlightRow {
    /// 客户端自铸的 id（8–40 字符）。
    pub id: String,
    /// 文章 id。
    pub article_id: i64,
    /// 所在正文对象 key。
    pub content_key: String,
    /// 原文侧还是译文侧。
    pub side: HighlightSide,
    /// 译文侧的语言；原文侧为 null。
    #[serde(default)]
    pub lang: Option<String>,
    /// 叶子节点 id（正文 HTML 的 `data-tb`）。
    pub leaf_id: String,
    /// 起点：叶子文本的 UTF-16 code unit 偏移。
    pub start: i64,
    /// 终点（必须大于 `start`）。
    pub end: i64,
    /// 选中文本（UTF-16 长度必须等于 `end - start`）。
    pub quote: String,
    /// 前文（≤32 字符）。
    pub prefix: String,
    /// 后文（≤32 字符）。
    pub suffix: String,
    /// 笔记；可空。
    #[serde(default)]
    pub note: Option<String>,
    /// 创建时间。
    pub created_at: i64,
    /// 更新时间。
    pub updated_at: i64,
    /// 删除时间；非空即已删。
    #[serde(default)]
    pub deleted_at: Option<i64>,
}

/// 译本行的状态（open enum）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranslationState {
    /// 尚未开始，排队中。
    Requested,
    /// 正在翻译。
    Running,
    /// 今天的额度用完，等到 `dueAt`。
    Waiting,
    /// 完成。
    Done,
    /// 部分块保留原文。
    Partial,
    /// 失败。
    Failed,
    /// 跳过。
    Skipped,
    /// 未知状态（v1 只做加法；按文档「未知按不可用处理」由调用方决定）。
    #[serde(other)]
    Unknown,
}

impl TranslationState {
    /// 是否正在进行（可以继续轮询）。
    pub fn is_running(self) -> bool {
        matches!(self, Self::Requested | Self::Running)
    }

    /// 是否已经产出了可读译本。
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Done | Self::Partial)
    }
}

/// `rows.translations`：某正文版本某语言的译本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationRow {
    /// 正文对象 key。
    pub content_key: String,
    /// 目标语言。
    pub lang: String,
    /// 状态。
    pub state: TranslationState,
    /// 已完成分块的路径（`/o/tc/…`）。
    #[serde(default)]
    pub chunks: Vec<String>,
    /// 完成后的译本对象路径（`/o/t/…`）；未完成时 null。
    #[serde(default)]
    pub object: Option<String>,
    /// `waiting` 状态下的开始时间（epoch 毫秒）。
    #[serde(default)]
    pub due_at: Option<i64>,
}

/// `rows.folders`：私有文件夹。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderRow {
    /// 客户端自铸的 id。
    pub id: String,
    /// 名称。
    pub name: String,
    /// 创建时间。
    pub created_at: i64,
    /// 更新时间。
    pub updated_at: i64,
    /// 删除时间；非空即已删。
    #[serde(default)]
    pub deleted_at: Option<i64>,
}

/// `rows.feedFolders`：feed 落在哪个文件夹。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedFolderRow {
    /// feed id。
    pub feed_id: i64,
    /// 文件夹 id；null 表示不在任何文件夹。
    #[serde(default)]
    pub folder_id: Option<String>,
    /// 最近一次移动的时间（晚者胜）。
    pub updated_at: i64,
}

/// `rows.follows`：关注的人。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowRow {
    /// 成员账号 id（`follow` / `unfollow` 用它）。
    pub user_id: String,
    /// handle。
    pub handle: String,
    /// 显示名；可空。
    #[serde(default)]
    pub display_name: Option<String>,
    /// 头像 URL；可空。
    #[serde(default)]
    pub avatar: Option<String>,
    /// 关注时间。
    pub created_at: i64,
    /// 取关时间；非空即已取关。
    #[serde(default)]
    pub deleted_at: Option<i64>,
}

/// `removed`：本页要求客户端删掉的行。今天只有 `articles`。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Removed {
    /// 要从设备上删除的文章 id。
    #[serde(default)]
    pub articles: Vec<i64>,
}

/// 各表的业务主键字段（wire 命名）。
///
/// 返回 `None` 表示不是 v1 的行表；`profile` 是单对象表，返回空切片表示固定主键。
pub fn key_fields(table: &str) -> Option<&'static [&'static str]> {
    Some(match table {
        "profile" => &[],
        "prefs" => &["key"],
        "subscriptions" => &["feedId"],
        "feeds" => &["id"],
        "sites" => &["id"],
        "articles" => &["id"],
        "titles" => &["articleId", "lang"],
        "states" => &["articleId"],
        "recommendations" => &["articleId"],
        "highlights" => &["id"],
        "translations" => &["contentKey", "lang"],
        "folders" => &["id"],
        "feedFolders" => &["feedId"],
        "follows" => &["userId"],
        _ => return None,
    })
}

/// 从一行 wire JSON 合成 `sync_rows.row_key`：多字段用 `/` 连接。
///
/// 返回 `None` 表示表名未知或主键字段缺失/类型不符——调用方应报协议错误。
pub fn row_key(table: &str, row: &Value) -> Option<String> {
    let fields = key_fields(table)?;
    if fields.is_empty() {
        return Some(ProfileRow::ROW_KEY.to_string());
    }
    let mut parts = Vec::with_capacity(fields.len());
    for field in fields {
        let part = match row.get(*field) {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Number(number)) => number.to_string(),
            _ => return None,
        };
        parts.push(part);
    }
    Some(parts.join("/"))
}

/// 便于测试与诊断：把 `SyncRows` 展开成 `(表名, 行 JSON 数组)`。
///
/// `profile` 若为 `Some` 会作为单元素数组返回。顺序固定为 [`SyncRows::TABLE_NAMES`]。
pub fn rows_as_tables(rows: &SyncRows) -> Vec<(&'static str, Vec<Value>)> {
    fn values<T: Serialize>(items: &[T]) -> Vec<Value> {
        items
            .iter()
            .map(|item| serde_json::to_value(item).unwrap_or(Value::Null))
            .collect()
    }

    let mut out = Vec::new();
    let mut push = |name: &'static str, items: Vec<Value>| {
        if !items.is_empty() {
            out.push((name, items));
        }
    };

    push(
        "profile",
        rows.profile
            .as_ref()
            .and_then(|profile| serde_json::to_value(profile).ok())
            .into_iter()
            .collect(),
    );
    push("prefs", values(&rows.prefs));
    push("subscriptions", values(&rows.subscriptions));
    push("feeds", values(&rows.feeds));
    push("sites", values(&rows.sites));
    push("articles", values(&rows.articles));
    push("titles", values(&rows.titles));
    push("states", values(&rows.states));
    push("recommendations", values(&rows.recommendations));
    push("highlights", values(&rows.highlights));
    push("translations", values(&rows.translations));
    push("folders", values(&rows.folders));
    push("feedFolders", values(&rows.feed_folders));
    push("follows", values(&rows.follows));
    out
}
