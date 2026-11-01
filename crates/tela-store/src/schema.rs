//! 本地库 DDL（schema v5，对应真实 API v1 + 本地网页收藏）。
//!
//! 与 v1（mock 协议）的差别：
//!
//! - `meta.value` 从 INTEGER 改为 TEXT：游标是不透明字符串，语言/类型也要存；
//! - `sync_rows` 去掉 `seq`：真实协议没有行序号，游标本身是不透明的；
//! - 新增 `pending_mutations`：离线队列必须跨进程存活，且要保留 `mid` 与 `at`；
//! - 新增 `bodies`：正文对象不可变，可以无限期缓存；
//! - 新增 `cached_translations`（v3）：译本对象也按 `(contentKey, lang)` 缓存，
//!   第一次加载后阅读面板不再重复向远端取译文；`state` 记录 pending/none 等快照，
//!   供离线打开时回退；
//! - 新增 `cached_images`（v4）：正文/译文里的图片按 URL 缓存原始字节，
//!   离线缓存按钮会连同图片一起下载；
//! - 新增 `bookmarks`（v5）：`Shift+B` 收藏的网页。它**不属于**同步行
//!   （`sync_rows`），退出登录/切换账号不会清掉用户自己收藏的网页；正文仍存
//!   `bodies`、图片仍存 `cached_images`，关掉应用再打开也能离线读；
//! - 新增 `documents`（v6）：收藏网页的正文以 **Markdown** 为规范格式存放
//!   （`bookmarks.content_key` → `documents.markdown`）。阅读时再渲染成块，
//!   图片仍按原始地址走 `cached_images`。

/// 当前 schema 的全部 DDL（幂等；旧库升级时只补新表）。
pub(crate) const SCHEMA_CURRENT: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_rows (
    entity  TEXT NOT NULL,
    row_key TEXT NOT NULL,
    body    TEXT NOT NULL,
    PRIMARY KEY (entity, row_key)
);

CREATE INDEX IF NOT EXISTS sync_rows_entity ON sync_rows (entity);

CREATE TABLE IF NOT EXISTS pending_mutations (
    seq  INTEGER PRIMARY KEY AUTOINCREMENT,
    mid  TEXT NOT NULL UNIQUE,
    at   INTEGER NOT NULL,
    body TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bodies (
    content_key TEXT PRIMARY KEY NOT NULL,
    body        TEXT NOT NULL,
    fetched_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS cached_translations (
    content_key TEXT NOT NULL,
    lang        TEXT NOT NULL,
    state       TEXT NOT NULL,
    object      TEXT,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (content_key, lang)
);

CREATE TABLE IF NOT EXISTS cached_images (
    src        TEXT PRIMARY KEY NOT NULL,
    body       BLOB NOT NULL,
    fetched_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS bookmarks (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    url         TEXT NOT NULL UNIQUE,
    title       TEXT NOT NULL,
    site        TEXT NOT NULL DEFAULT '',
    lang        TEXT NOT NULL DEFAULT '',
    excerpt     TEXT NOT NULL DEFAULT '',
    content_key TEXT NOT NULL,
    image_count INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS bookmarks_created_at ON bookmarks (created_at DESC, id DESC);

CREATE TABLE IF NOT EXISTS documents (
    content_key TEXT PRIMARY KEY NOT NULL,
    markdown    TEXT NOT NULL,
    lang        TEXT NOT NULL DEFAULT '',
    updated_at  INTEGER NOT NULL
);
";
