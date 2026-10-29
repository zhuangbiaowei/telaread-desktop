//! `tela-proto` — Tela API v1 的 wire 类型、常量与纯逻辑。
//!
//! 职责边界：只放 wire 类型、协议常量和无 I/O 的判定规则，不发网络、不开库、不碰 UI。
//!
//! ## 依据
//!
//! 全部形状来自官方文档与 OpenAPI（2026-10-28 实读）：
//!
//! | 内容 | 依据 |
//! |---|---|
//! | 认证、令牌、错误 | [`docs/api/authorization.md`](../../../docs/api/authorization.md) |
//! | 同步、变更、正文、翻译 | [`docs/api/v1.md`](../../../docs/api/v1.md) |
//! | 字段级 schema | `docs/api/openapi.json`（本地副本 `tmp/api/openapi.json`） |
//!
//! 与旧版（mock 协议）的差别是**换代**而不是演进：cursor 变成不透明字符串、行不再带
//! `seq`、没有 tombstone（删除由 `deletedAt` 与 `removed.articles` 表达）、时间一律
//! epoch **毫秒**、mutation 由 17 种组成。旧类型全部删除，避免两套协议并存。

/// v1 API 版本号（每个响应头的 `Tela-API-Version`）。
pub const API_VERSION: u32 = 1;

/// 生产 API 基址。
pub const DEFAULT_API_BASE_URL: &str = "https://api.telaread.com";

/// 浏览器里的授权端点所在的站点（与 API 不同源：需要成员的登录会话）。
pub const DEFAULT_AUTH_BASE_URL: &str = "https://telaread.com";

/// 授权服务器的 issuer（RFC 9207 校验用，必须逐字相等）。
pub const ISSUER: &str = "https://api.telaread.com";

/// 一次 push 最多携带的 mutation 条数（`limits.pushMutations`）。
pub const MAX_MUTATIONS_PER_PUSH: usize = 50;

/// 一次 push 请求体的字节上限（`limits.pushBytes`，1 MB）。
pub const MAX_PUSH_BYTES: usize = 1_048_576;

/// 一次 `/v1/bodies` 最多请求的正文 key 数（`limits.bodiesPerRequest`）。
pub const MAX_BODIES_PER_REQUEST: usize = 25;

/// 阅读地平线（天）：早于该天数的文章一律视为已读（`limits.horizonDays`）。
pub const DEFAULT_HORIZON_DAYS: i64 = 30;

/// 当前支持的阅读语言。
pub const READING_LANGUAGES: [&str; 4] = ["zh-Hans", "zh-Hant", "en", "fr"];

/// 默认同步行类型（与 `/v1/sync` 不传 `types` 时服务端给出的一致）。
///
/// 文档保证这 14 张表在 v1 内不变；以后新增的类型只有显式点名才会下发。
pub const DEFAULT_SYNC_TYPES: [&str; 14] = [
    "profile",
    "prefs",
    "subscriptions",
    "feeds",
    "sites",
    "articles",
    "titles",
    "states",
    "recommendations",
    "highlights",
    "translations",
    "folders",
    "feedFolders",
    "follows",
];

/// 生成一个 mutation id（8–40 字符）：UUID v7 的规范文本恰好 36 字符。
pub fn new_mid() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// 当前时间（epoch 毫秒）；系统时钟早于 1970 时退化为 0，不 panic。
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

mod account;
mod error;
mod mutation;
mod read;
mod rows;
mod sync;

pub use account::{ApiVersionInfo, Limits, Me, Meta, OauthMetadata, Usage};
pub use error::{ProtocolStage, StoreStage, TelaError, WireContext};
pub use mutation::{
    MAX_HIGHLIGHT_NOTE, MAX_HIGHLIGHT_QUOTE, MAX_RECOMMENDATION_NOTE, Mutation, MutationBase,
    MutationResult, MutationStatus, PushRequest, PushResponse, validate_push,
};
pub use read::{ReadRuleInput, is_read, read_rule};
pub use rows::{
    ArticleRow, FeedFolderRow, FeedRow, FolderRow, FollowRow, HighlightRow, HighlightSide, PrefRow,
    ProfileRow, RecommendationRow, Removed, SiteRow, StateRow, SubscriptionRow, SyncRows, TitleRow,
    TranslationRow, TranslationState, key_fields, row_key, rows_as_tables,
};
pub use sync::SyncResponse;
