//! `tela-store` — SQLite schema、迁移、同步落库、离线变更队列与正文缓存。
//!
//! 职责边界：不决定「何时拉取」、不渲染、不发网络。
//!
//! ## 持久化形态
//!
//! 行数据按「wire 表名 + 业务主键 + JSON 正文」单表存放（`sync_rows`），不手写 14 套列映射：
//!
//! - 字段清单的唯一真相是 `tela-proto::rows`（对齐 OpenAPI），列映射会随协议演进重复漂移；
//! - 写入与读取共用同一套 serde 结构，不需要第二份字段清单；
//! - 幂等去重由 `PRIMARY KEY (entity, row_key)` 保证，与字段多少无关。
//!
//! 查询需要按列过滤/排序时（例如按 `sortAt` 做时间线），在 `Store` 内部把 entity 拆成
//! 列式表即可，公开 API 不变。
//!
//! ## 版本
//!
//! [`LATEST_SCHEMA_VERSION`] = 6。v1 是旧 mock 协议（行带 `seq`、游标是整数），
//! 迁移时丢弃其行数据并在 README 说明；真实 API 的数据从登录后的第一次快照开始。
//! v3–v6 只补表（译本缓存、图片缓存、本地网页收藏、Markdown 正文），升级不丢数据。

mod schema;
mod store;

pub use store::{
    ApplyOutcome, BookmarkInput, BookmarkRow, CachedDocument, CachedTranslation,
    LATEST_SCHEMA_VERSION, PROFILE_ROW_KEY, Store, StoredRow,
};
