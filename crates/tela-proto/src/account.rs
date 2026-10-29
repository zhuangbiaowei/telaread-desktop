//! 账号与能力发现：`/v1/me`、`/v1/usage`、`/meta`。
//!
//! 这三份响应的作用不同：
//! - `/v1/me`：当前令牌「是谁、能做什么」（`scope` 是此刻的，不是签发时的）；
//! - `/v1/usage`：今天还剩多少额度（翻译、新 feed、订阅数、非订阅文章）；
//! - `/meta`：服务端现在提供哪些功能与上限，启动时先看它再决定依赖什么。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// `GET /v1/me`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    /// 唯一 handle。
    pub handle: String,
    /// 显示名；可空。
    #[serde(default)]
    pub display_name: Option<String>,
    /// 头像绝对 URL；可空。
    #[serde(default)]
    pub avatar: Option<String>,
    /// 阅读语言；`null` 表示跟随界面语言。
    #[serde(default)]
    pub reading_language: Option<String>,
    /// 这个令牌**现在**的权限：`read` 或 `write`。
    pub scope: String,
    /// 只有在成员允许应用查看邮箱时才出现。
    #[serde(default)]
    pub email: Option<String>,
}

impl Me {
    /// 当前令牌是否可写。
    pub fn can_write(&self) -> bool {
        self.scope == "write"
    }
}

/// 一个可空的每日额度：`null` 表示不限额，`0` 表示没有。
pub type Quota = Option<i64>;

/// `GET /v1/usage`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// 翻译额度。
    pub translations: TranslationUsage,
    /// 新 feed 额度。
    pub feeds: FeedUsage,
    /// 订阅上限。
    pub subscriptions: SubscriptionUsage,
    /// 非订阅博客的正文额度。
    pub posts: PostUsage,
    /// 本应用每分钟的请求限额。
    pub rate_limits: RateLimits,
}

/// 翻译额度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationUsage {
    /// 每日上限；null 表示不限。
    pub per_day: Quota,
    /// 今天已用。
    pub used_today: i64,
    /// 被推到明天做的数量。
    pub waiting: i64,
    /// 下一次重置（UTC 零点，epoch 毫秒）。
    pub resets_at: i64,
}

/// 新 feed 额度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedUsage {
    /// 每日上限；null 表示不限。
    pub per_day: Quota,
    /// 今天新增。
    pub added_today: i64,
    /// 导入溢出、将在之后几天加入的数量。
    pub pending: i64,
}

/// 订阅上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionUsage {
    /// 上限；null 表示不限。
    pub max: Quota,
    /// 当前订阅数。
    pub count: i64,
}

/// 非订阅博客的正文额度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostUsage {
    /// 每日上限；null 表示不限。
    pub per_day: Quota,
    /// 今天已打开。
    pub opened_today: i64,
    /// 下一次重置（epoch 毫秒）。
    pub resets_at: i64,
}

/// 应用的速率限额（每分钟）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimits {
    /// 每个成员每分钟。
    pub per_member_per_minute: Quota,
    /// 全体成员合计每分钟。
    pub per_app_per_minute: Quota,
}

/// `GET /meta`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    /// API 名（`tela`）。
    pub api: String,
    /// 最新版本。
    pub current: u32,
    /// 仍可用的最老版本。
    pub oldest: u32,
    /// 各版本的状态。
    #[serde(default)]
    pub versions: Vec<ApiVersionInfo>,
    /// 版本 → 功能名。
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,
    /// 本次的上限。
    pub limits: Limits,
    /// OAuth 元数据地址。
    pub oauth: OauthMetadata,
}

impl Meta {
    /// v1 是否仍然可用。
    pub fn supports_v1(&self) -> bool {
        self.current >= 1 && self.oldest <= 1
    }

    /// v1 是否提供某个功能。
    pub fn has_feature(&self, feature: &str) -> bool {
        self.features
            .get("v1")
            .is_some_and(|features| features.iter().any(|name| name == feature))
    }
}

/// 一个 API 版本的状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiVersionInfo {
    /// 版本号。
    pub version: u32,
    /// 状态（open enum：`preview` / `stable` / `deprecated` / …）。
    pub status: String,
    /// 停止服务的时间；未计划时为 null。
    #[serde(default)]
    pub sunset_at: Option<i64>,
}

/// `/meta.limits`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// 一次 push 的变更条数上限。
    pub push_mutations: i64,
    /// 一次 push 的字节上限。
    pub push_bytes: i64,
    /// 一页同步的行数上限。
    pub page_rows: i64,
    /// 一次 `/v1/bodies` 的 key 数上限。
    pub bodies_per_request: i64,
    /// 阅读地平线（天）。
    pub horizon_days: i64,
    /// 阅读语言。
    #[serde(default)]
    pub reading_languages: Vec<String>,
    /// 每日翻译上限。
    #[serde(default)]
    pub translations_per_day: Quota,
    /// 每日新 feed 上限。
    #[serde(default)]
    pub feeds_per_day: Quota,
    /// 订阅上限。
    #[serde(default)]
    pub max_subscriptions: Quota,
    /// 每日非订阅文章上限。
    #[serde(default)]
    pub unsubscribed_posts_per_day: Quota,
}

/// `/meta.oauth`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OauthMetadata {
    /// RFC 8414 元数据地址。
    pub metadata: String,
}
