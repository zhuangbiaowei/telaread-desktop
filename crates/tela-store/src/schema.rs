//! 本地库 DDL（schema v2，对应真实 API v1）。
//!
//! 与 v1（mock 协议）的差别：
//!
//! - `meta.value` 从 INTEGER 改为 TEXT：游标是不透明字符串，语言/类型也要存；
//! - `sync_rows` 去掉 `seq`：真实协议没有行序号，游标本身是不透明的；
//! - 新增 `pending_mutations`：离线队列必须跨进程存活，且要保留 `mid` 与 `at`；
//! - 新增 `bodies`：正文对象不可变，可以无限期缓存。

/// schema v2 的全部 DDL（幂等）。
pub(crate) const SCHEMA_V2: &str = "
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
";
