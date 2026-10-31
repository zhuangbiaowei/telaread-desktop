//! 本地库：打开、迁移（v1 mock 协议 → v2 真实 API）、行/游标/队列/正文缓存读写。
//!
//! 设计取舍沿用 v1：行数据按 `(entity, row_key)` 存 wire JSON，表结构演进只需加迁移，
//! 不必为 14 张 wire 表各写一套列映射。真实协议下主键仍然是业务键（文章 id、
//! `(articleId, lang)` 等），只是少了 `seq`。

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use tela_proto::{
    Mutation, ProfileRow, StoreStage, SyncResponse, TelaError, row_key, rows_as_tables,
};

use crate::schema;

/// 当前 schema 版本。
///
/// v3 新增 `cached_translations`，v4 新增 `cached_images`；旧库升级只补表，
/// 不会丢正文/译文缓存与离线队列。
pub const LATEST_SCHEMA_VERSION: i64 = 4;

const META_SCHEMA_VERSION: &str = "schema_version";
const META_SYNC_CURSOR: &str = "sync_cursor";
const META_SYNC_LANG: &str = "sync_lang";
const META_SYNC_TYPES: &str = "sync_types";
const META_ACCOUNT: &str = "account_handle";

/// 本地库句柄。所有写操作走事务，失败即回滚。
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    path: PathBuf,
    schema_version: i64,
}

/// 构造库阶段错误。
pub(crate) fn store_error(stage: StoreStage, path: &Path) -> TelaError {
    TelaError::Store {
        stage,
        path: path.display().to_string(),
    }
}

impl Store {
    /// 打开（必要时初始化）库文件。
    pub fn open(path: &Path) -> Result<Self, TelaError> {
        let conn = Connection::open(path).map_err(|_| store_error(StoreStage::Open, path))?;
        Self::from_connection(conn, path)
    }

    /// 打开内存库（测试用）。
    pub fn open_in_memory() -> Result<Self, TelaError> {
        let label = Path::new(":memory:");
        let conn =
            Connection::open_in_memory().map_err(|_| store_error(StoreStage::Open, label))?;
        Self::from_connection(conn, label)
    }

    fn from_connection(conn: Connection, path: &Path) -> Result<Self, TelaError> {
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|_| store_error(StoreStage::Open, path))?;
        let mut store = Self {
            conn,
            path: path.to_path_buf(),
            schema_version: 0,
        };
        store.migrate()?;
        Ok(store)
    }

    /// v1 → 当前版本迁移：旧库是 mock 协议（行带 `seq`、游标是整数），与真实 API 不兼容。
    ///
    /// 这里**丢弃**旧的 `sync_rows` 与 `meta`，但保留用户目录里的库文件本身。理由：
    /// v1 数据全部来自 mock/演示，没有任何真实服务端数据可保留；而 `pending_mutations`
    /// 是 v2 新表，不受影响（旧库本来也没有）。
    fn migrate(&mut self) -> Result<(), TelaError> {
        let path = self.path.clone();
        let migrate_err = || store_error(StoreStage::Migrate, &path);

        let current = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [META_SCHEMA_VERSION],
                |row| {
                    // v1 的 value 列是 INTEGER，v2 是 TEXT：两种都要读得出来。
                    row.get::<_, rusqlite::types::Value>(0)
                        .map(|value| match value {
                            rusqlite::types::Value::Integer(number) => number,
                            rusqlite::types::Value::Text(text) => text.parse().unwrap_or(0),
                            _ => 0,
                        })
                },
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or(0);

        if current > LATEST_SCHEMA_VERSION {
            return Err(migrate_err());
        }

        let tx = self.conn.transaction().map_err(|_| migrate_err())?;
        if current == 1 {
            tx.execute_batch("DROP TABLE IF EXISTS sync_rows; DROP TABLE IF EXISTS meta;")
                .map_err(|_| migrate_err())?;
        }
        tx.execute_batch(schema::SCHEMA_CURRENT)
            .map_err(|_| migrate_err())?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![META_SCHEMA_VERSION, LATEST_SCHEMA_VERSION.to_string()],
        )
        .map_err(|_| migrate_err())?;
        tx.commit().map_err(|_| migrate_err())?;

        self.schema_version = LATEST_SCHEMA_VERSION;
        Ok(())
    }

    pub(crate) fn connection(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// 当前 schema 版本。
    pub fn schema_version(&self) -> i64 {
        self.schema_version
    }

    /// 库文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    // ---- meta 键值 ----

    /// 读一个 meta 键。
    pub fn meta(&self, key: &str) -> Result<Option<String>, TelaError> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|_| store_error(StoreStage::Query, &self.path))
    }

    /// 写一个 meta 键。
    pub fn set_meta(&mut self, key: &str, value: &str) -> Result<(), TelaError> {
        self.conn
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(())
    }

    /// 同步游标；从未同步过或已失效时为 `None`。
    pub fn cursor(&self) -> Result<Option<String>, TelaError> {
        Ok(self
            .meta(META_SYNC_CURSOR)?
            .filter(|cursor| !cursor.is_empty()))
    }

    /// 游标对应的语言。
    pub fn sync_lang(&self) -> Result<Option<String>, TelaError> {
        self.meta(META_SYNC_LANG)
    }

    /// 游标覆盖的行类型。
    pub fn sync_types(&self) -> Result<Vec<String>, TelaError> {
        let Some(raw) = self.meta(META_SYNC_TYPES)? else {
            return Ok(Vec::new());
        };
        Ok(serde_json::from_str(&raw).unwrap_or_default())
    }

    /// 已登录成员的 handle（不是秘密，可以放本地库）。
    pub fn account(&self) -> Result<Option<String>, TelaError> {
        self.meta(META_ACCOUNT)
    }

    /// 记录已登录成员。
    pub fn set_account(&mut self, handle: &str) -> Result<(), TelaError> {
        self.set_meta(META_ACCOUNT, handle)
    }

    /// 清掉游标、语言与类型（下一次同步从快照开始）。
    pub fn reset_sync_state(&mut self) -> Result<(), TelaError> {
        self.conn
            .execute(
                "DELETE FROM meta WHERE key IN (?1, ?2, ?3)",
                params![META_SYNC_CURSOR, META_SYNC_LANG, META_SYNC_TYPES],
            )
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(())
    }

    /// 一次性写入游标、语言与类型（与行数据同一事务时由 `apply_sync` 负责）。
    pub fn set_sync_state(
        &mut self,
        cursor: &str,
        lang: &str,
        types: &[String],
    ) -> Result<(), TelaError> {
        let encoded = serde_json::to_string(types).unwrap_or_else(|_| "[]".to_string());
        self.set_meta(META_SYNC_CURSOR, cursor)?;
        self.set_meta(META_SYNC_LANG, lang)?;
        self.set_meta(META_SYNC_TYPES, &encoded)
    }

    // ---- 行数据 ----

    /// 写一行（按 `(entity, row_key)` 覆盖）。
    pub fn upsert_row(&mut self, entity: &str, key: &str, body: &str) -> Result<(), TelaError> {
        self.conn
            .execute(
                "INSERT INTO sync_rows (entity, row_key, body) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(entity, row_key) DO UPDATE SET body = excluded.body",
                params![entity, key, body],
            )
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(())
    }

    /// 按主键读一行。
    pub fn get_row(&self, entity: &str, key: &str) -> Result<Option<StoredRow>, TelaError> {
        self.conn
            .query_row(
                "SELECT entity, row_key, body FROM sync_rows WHERE entity = ?1 AND row_key = ?2",
                params![entity, key],
                |row| {
                    Ok(StoredRow {
                        entity: row.get(0)?,
                        row_key: row.get(1)?,
                        body: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(|_| store_error(StoreStage::Query, &self.path))
    }

    /// 按 wire 表列出全部行，`row_key` 升序。
    pub fn rows(&self, entity: &str) -> Result<Vec<StoredRow>, TelaError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT entity, row_key, body FROM sync_rows \
                 WHERE entity = ?1 ORDER BY row_key ASC",
            )
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        let mapped = stmt
            .query_map(params![entity], |row| {
                Ok(StoredRow {
                    entity: row.get(0)?,
                    row_key: row.get(1)?,
                    body: row.get(2)?,
                })
            })
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        mapped
            .collect::<Result<Vec<StoredRow>, _>>()
            .map_err(|_| store_error(StoreStage::Query, &self.path))
    }

    /// 把某个表的全部行解成结构体；单行解码失败按协议违约报错。
    pub fn rows_as<T>(&self, entity: &str) -> Result<Vec<T>, TelaError>
    where
        T: serde::de::DeserializeOwned,
    {
        self.rows(entity)?
            .into_iter()
            .map(|row| {
                serde_json::from_str(&row.body).map_err(|_| TelaError::Protocol {
                    stage: tela_proto::ProtocolStage::ShapeMismatch,
                    object: format!("{entity}/{}", row.row_key),
                })
            })
            .collect()
    }

    /// 按主键读一行并解码。
    pub fn get_as<T>(&self, entity: &str, key: &str) -> Result<Option<T>, TelaError>
    where
        T: serde::de::DeserializeOwned,
    {
        let Some(row) = self.get_row(entity, key)? else {
            return Ok(None);
        };
        serde_json::from_str(&row.body)
            .map(Some)
            .map_err(|_| TelaError::Protocol {
                stage: tela_proto::ProtocolStage::ShapeMismatch,
                object: format!("{entity}/{key}"),
            })
    }

    /// 某个表的行数。
    pub fn row_count(&self, entity: &str) -> Result<usize, TelaError> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sync_rows WHERE entity = ?1",
                params![entity],
                |row| row.get(0),
            )
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// 删除一行；返回此前是否存在。
    pub fn delete_row(&mut self, entity: &str, key: &str) -> Result<bool, TelaError> {
        let affected = self
            .conn
            .execute(
                "DELETE FROM sync_rows WHERE entity = ?1 AND row_key = ?2",
                params![entity, key],
            )
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(affected > 0)
    }

    /// 清空该账号的全部同步数据（登出时用），保留正文缓存。
    pub fn clear_account(&mut self) -> Result<(), TelaError> {
        let path = self.path.clone();
        let tx = self
            .connection()
            .transaction()
            .map_err(|_| store_error(StoreStage::Query, &path))?;
        tx.execute("DELETE FROM sync_rows", [])
            .map_err(|_| store_error(StoreStage::Query, &path))?;
        tx.execute("DELETE FROM pending_mutations", [])
            .map_err(|_| store_error(StoreStage::Query, &path))?;
        tx.execute(
            "DELETE FROM meta WHERE key IN (?1, ?2, ?3, ?4)",
            params![
                META_SYNC_CURSOR,
                META_SYNC_LANG,
                META_SYNC_TYPES,
                META_ACCOUNT
            ],
        )
        .map_err(|_| store_error(StoreStage::Query, &path))?;
        tx.commit()
            .map_err(|_| store_error(StoreStage::Query, &path))?;
        Ok(())
    }

    // ---- 离线变更队列 ----

    /// 入队；同一 `mid` 重复入队是幂等的（返回 `false`）。
    pub fn enqueue_mutation(&mut self, mutation: &Mutation) -> Result<bool, TelaError> {
        let body = serde_json::to_string(mutation).map_err(|err| TelaError::Config {
            detail: format!("变更无法序列化：{err}"),
        })?;
        let affected = self
            .conn
            .execute(
                "INSERT OR IGNORE INTO pending_mutations (mid, at, body) VALUES (?1, ?2, ?3)",
                params![mutation.mid(), mutation.at(), body],
            )
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(affected > 0)
    }

    /// 取最早的 `limit` 条（按入队顺序）。
    pub fn pending_mutations(&self, limit: usize) -> Result<Vec<Mutation>, TelaError> {
        let mut stmt = self
            .conn
            .prepare("SELECT body FROM pending_mutations ORDER BY seq ASC LIMIT ?1")
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        let mapped = stmt
            .query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        mapped
            .collect::<Result<Vec<String>, _>>()
            .map_err(|_| store_error(StoreStage::Query, &self.path))?
            .into_iter()
            .map(|body| {
                serde_json::from_str(&body).map_err(|_| TelaError::Protocol {
                    stage: tela_proto::ProtocolStage::ShapeMismatch,
                    object: "pending_mutations.body".to_string(),
                })
            })
            .collect()
    }

    /// 待发条数。
    pub fn pending_count(&self) -> Result<usize, TelaError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM pending_mutations", [], |row| {
                row.get(0)
            })
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// 删除一条已答复的变更；返回是否存在。
    pub fn delete_mutation(&mut self, mid: &str) -> Result<bool, TelaError> {
        let affected = self
            .conn
            .execute("DELETE FROM pending_mutations WHERE mid = ?1", params![mid])
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(affected > 0)
    }

    /// 批量删除已答复的变更。
    pub fn delete_mutations(&mut self, mids: &[String]) -> Result<usize, TelaError> {
        let path = self.path.clone();
        let tx = self
            .connection()
            .transaction()
            .map_err(|_| store_error(StoreStage::Query, &path))?;
        let mut deleted = 0usize;
        {
            let mut stmt = tx
                .prepare("DELETE FROM pending_mutations WHERE mid = ?1")
                .map_err(|_| store_error(StoreStage::Query, &path))?;
            for mid in mids {
                deleted += stmt
                    .execute(params![mid])
                    .map_err(|_| store_error(StoreStage::Query, &path))?;
            }
        }
        tx.commit()
            .map_err(|_| store_error(StoreStage::Query, &path))?;
        Ok(deleted)
    }

    // ---- 正文缓存 ----

    /// 缓存一个不可变正文对象（JSON）。
    pub fn put_body(&mut self, content_key: &str, body: &str) -> Result<(), TelaError> {
        self.conn
            .execute(
                "INSERT INTO bodies (content_key, body, fetched_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(content_key) DO UPDATE SET body = excluded.body, \
                 fetched_at = excluded.fetched_at",
                params![content_key, body, tela_proto::now_ms()],
            )
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(())
    }

    /// 读一个缓存的正文对象。
    pub fn get_body(&self, content_key: &str) -> Result<Option<String>, TelaError> {
        self.conn
            .query_row(
                "SELECT body FROM bodies WHERE content_key = ?1",
                params![content_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| store_error(StoreStage::Query, &self.path))
    }

    /// 缓存的正文对象数。
    pub fn body_count(&self) -> Result<usize, TelaError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM bodies", [], |row| row.get(0))
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    // ---- 译文缓存 ----

    /// 写入一个译本快照（`state` 是 wire 小写状态；`object` 是译本对象 JSON）。
    ///
    /// 同一 `(content_key, lang)` 覆盖写：服务端从 pending 变成 done 时只更新这一行。
    pub fn put_translation(
        &mut self,
        content_key: &str,
        lang: &str,
        state: &str,
        object: Option<&str>,
    ) -> Result<(), TelaError> {
        self.conn
            .execute(
                "INSERT INTO cached_translations (content_key, lang, state, object, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(content_key, lang) DO UPDATE SET
                   state = excluded.state,
                   object = excluded.object,
                   updated_at = excluded.updated_at",
                params![content_key, lang, state, object, tela_proto::now_ms()],
            )
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(())
    }

    /// 读一个译本快照；没有缓存过返回 `None`。
    pub fn get_translation(
        &self,
        content_key: &str,
        lang: &str,
    ) -> Result<Option<CachedTranslation>, TelaError> {
        self.conn
            .query_row(
                "SELECT content_key, lang, state, object, updated_at \
                 FROM cached_translations WHERE content_key = ?1 AND lang = ?2",
                params![content_key, lang],
                |row| {
                    Ok(CachedTranslation {
                        content_key: row.get(0)?,
                        lang: row.get(1)?,
                        state: row.get(2)?,
                        object: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|_| store_error(StoreStage::Query, &self.path))
    }

    /// 缓存的译本快照数。
    pub fn translation_count(&self) -> Result<usize, TelaError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM cached_translations", [], |row| {
                row.get(0)
            })
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// 某个正文 key 下缓存的译本快照数（多语言）。
    pub fn translation_count_for(&self, content_key: &str) -> Result<usize, TelaError> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM cached_translations WHERE content_key = ?1",
                params![content_key],
                |row| row.get(0),
            )
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    // ---- 图片缓存 ----

    /// 缓存一张图片的原始字节（按 URL/src 覆盖写；图片是不可变对象）。
    pub fn put_image(&mut self, src: &str, body: &[u8]) -> Result<(), TelaError> {
        self.conn
            .execute(
                "INSERT INTO cached_images (src, body, fetched_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(src) DO UPDATE SET body = excluded.body, \
                 fetched_at = excluded.fetched_at",
                params![src, body, tela_proto::now_ms()],
            )
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(())
    }

    /// 读缓存的图片字节；没有缓存过返回 `None`。
    pub fn get_image(&self, src: &str) -> Result<Option<Vec<u8>>, TelaError> {
        self.conn
            .query_row(
                "SELECT body FROM cached_images WHERE src = ?1",
                params![src],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| store_error(StoreStage::Query, &self.path))
    }

    /// 缓存的图片数。
    pub fn image_count(&self) -> Result<usize, TelaError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM cached_images", [], |row| row.get(0))
            .map_err(|_| store_error(StoreStage::Query, &self.path))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }
}

/// 一条缓存的译本快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedTranslation {
    /// 正文 key。
    pub content_key: String,
    /// 译文语言。
    pub lang: String,
    /// 最近一次观察到的 wire 状态（`none` / `requested` / `running` / `waiting` / `done` / `partial` …）。
    pub state: String,
    /// 完成时的译本对象 JSON（`state` 为 pending/none 时为 `None`）。
    pub object: Option<String>,
    /// 写入时间（epoch 毫秒）。
    pub updated_at: i64,
}

/// 一条已落库的同步行：wire 表名 + 业务主键 + wire JSON 正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRow {
    /// wire 表名。
    pub entity: String,
    /// 业务主键的字符串形式（多字段用 `/` 连接）。
    pub row_key: String,
    /// wire JSON。
    pub body: String,
}

/// 一页同步的落库结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyOutcome {
    /// 写入/覆盖的行数。
    pub rows: usize,
    /// 因 `removed.articles` 删除的文章数。
    pub removed: usize,
    /// 因退订/翻译开关而级联删除的行数。
    pub cascaded: usize,
    /// 是否是快照页。
    pub reset: bool,
    /// 落库后的游标。
    pub cursor: String,
}

impl Store {
    /// 把一页 `/v1/sync` 合并进本地库（单事务）。
    ///
    /// 语义严格按文档：
    /// - `reset: true`：先清空该账号全部行，再写入本页；
    /// - `removed.articles`：删文章，并级联删掉它的标题/状态/推荐/正文译本；
    /// - 退订（`subscriptions.deletedAt`）：删该 feed 的文章，**但保留**被点赞、被推荐、
    ///   有划线的文章；
    /// - `sites.translationAllowed == false`：删该站文章的全部标题与正文译本。
    pub fn apply_sync(&mut self, page: &SyncResponse) -> Result<ApplyOutcome, TelaError> {
        let path = self.path().to_path_buf();
        let mut outcome = ApplyOutcome {
            reset: page.reset,
            cursor: page.cursor.clone(),
            ..ApplyOutcome::default()
        };

        let tx = self
            .connection()
            .transaction()
            .map_err(|_| store_error(StoreStage::Query, &path))?;

        if page.reset {
            tx.execute("DELETE FROM sync_rows", [])
                .map_err(|_| store_error(StoreStage::Query, &path))?;
        }

        for (table, rows) in rows_as_tables(&page.rows) {
            for (index, row) in rows.iter().enumerate() {
                let key = row_key(table, row).ok_or_else(|| TelaError::Protocol {
                    stage: tela_proto::ProtocolStage::ShapeMismatch,
                    object: format!("rows.{table}[{index}]"),
                })?;
                tx.execute(
                    "INSERT INTO sync_rows (entity, row_key, body) VALUES (?1, ?2, ?3)
                     ON CONFLICT(entity, row_key) DO UPDATE SET body = excluded.body",
                    params![table, key, row.to_string()],
                )
                .map_err(|_| store_error(StoreStage::Query, &path))?;
                outcome.rows += 1;
            }
        }

        for article_id in &page.removed.articles {
            outcome.removed += remove_article(&tx, &path, *article_id)?;
        }

        for subscription in &page.rows.subscriptions {
            if subscription.deleted_at.is_some() {
                outcome.cascaded += trim_unsubscribed_feed(&tx, &path, subscription.feed_id)?;
            }
        }

        for site in &page.rows.sites {
            if !site.translation_allowed {
                outcome.cascaded += drop_translations_of_site(&tx, &path, site.id)?;
            }
        }

        // 游标、语言、类型与行同一事务提交：断点续跑时三者永远一致。
        let types = serde_json::to_string(&page.types).unwrap_or_else(|_| "[]".to_string());
        for (key, value) in [
            (META_SYNC_CURSOR, page.cursor.as_str()),
            (META_SYNC_LANG, page.lang.as_str()),
            (META_SYNC_TYPES, types.as_str()),
        ] {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|_| store_error(StoreStage::Query, &path))?;
        }

        tx.commit()
            .map_err(|_| store_error(StoreStage::Query, &path))?;
        Ok(outcome)
    }
}

/// 删一篇设备上的文章及其派生行；返回删除的文章数（0 或 1）。
fn remove_article(
    tx: &rusqlite::Transaction<'_>,
    path: &Path,
    article_id: i64,
) -> Result<usize, TelaError> {
    let content_key: Option<String> = tx
        .query_row(
            "SELECT body FROM sync_rows WHERE entity = 'articles' AND row_key = ?1",
            params![article_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| store_error(StoreStage::Query, path))?
        .and_then(|body| {
            serde_json::from_str::<tela_proto::ArticleRow>(&body)
                .ok()
                .and_then(|article| article.content_key)
        });

    let key = article_id.to_string();
    tx.execute(
        "DELETE FROM sync_rows WHERE (entity = 'articles' AND row_key = ?1) \
         OR (entity IN ('states', 'recommendations') AND row_key = ?1) \
         OR (entity = 'titles' AND row_key LIKE ?2)",
        params![key, format!("{article_id}/%")],
    )
    .map_err(|_| store_error(StoreStage::Query, path))?;

    if let Some(content_key) = content_key {
        tx.execute(
            "DELETE FROM sync_rows WHERE entity = 'translations' AND row_key LIKE ?1",
            params![format!("{content_key}/%")],
        )
        .map_err(|_| store_error(StoreStage::Query, path))?;
    }
    Ok(1)
}

/// 退订后清理该 feed 的文章，保留「被成员留下」的那些。
///
/// 文档定义的「keep」：点赞过、推荐未撤回、有未删除的划线。
fn trim_unsubscribed_feed(
    tx: &rusqlite::Transaction<'_>,
    path: &Path,
    feed_id: i64,
) -> Result<usize, TelaError> {
    let mut stmt = tx
        .prepare("SELECT row_key, body FROM sync_rows WHERE entity = 'articles'")
        .map_err(|_| store_error(StoreStage::Query, path))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| store_error(StoreStage::Query, path))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| store_error(StoreStage::Query, path))?;
    drop(stmt);

    let mut removed = 0usize;
    for (_key, body) in rows {
        let Ok(article) = serde_json::from_str::<tela_proto::ArticleRow>(&body) else {
            continue;
        };
        if article.feed_id != feed_id {
            continue;
        }
        if is_kept(tx, path, &article)? {
            continue;
        }
        removed += remove_article(tx, path, article.id)?;
    }
    Ok(removed)
}

/// 成员是否「留下」了这篇文章。
fn is_kept(
    tx: &rusqlite::Transaction<'_>,
    path: &Path,
    article: &tela_proto::ArticleRow,
) -> Result<bool, TelaError> {
    let key = article.id.to_string();
    let state: Option<String> = tx
        .query_row(
            "SELECT body FROM sync_rows WHERE entity = 'states' AND row_key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| store_error(StoreStage::Query, path))?;
    if state
        .as_deref()
        .and_then(|body| serde_json::from_str::<tela_proto::StateRow>(body).ok())
        .is_some_and(|state| state.liked_at.is_some())
    {
        return Ok(true);
    }

    let recommendation: Option<String> = tx
        .query_row(
            "SELECT body FROM sync_rows WHERE entity = 'recommendations' AND row_key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| store_error(StoreStage::Query, path))?;
    if recommendation
        .as_deref()
        .and_then(|body| serde_json::from_str::<tela_proto::RecommendationRow>(body).ok())
        .is_some_and(|recommendation| recommendation.deleted_at.is_none())
    {
        return Ok(true);
    }

    let highlighted: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM sync_rows WHERE entity = 'highlights' AND \
             json_extract(body, '$.articleId') = ?1 AND \
             json_extract(body, '$.deletedAt') IS NULL LIMIT 1",
            params![article.id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| store_error(StoreStage::Query, path))?;
    Ok(highlighted.is_some())
}

/// 博客关闭翻译后，删掉它所有文章的标题译文与正文译本。
fn drop_translations_of_site(
    tx: &rusqlite::Transaction<'_>,
    path: &Path,
    site_id: i64,
) -> Result<usize, TelaError> {
    let mut stmt = tx
        .prepare("SELECT body FROM sync_rows WHERE entity = 'feeds'")
        .map_err(|_| store_error(StoreStage::Query, path))?;
    let feed_ids = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| store_error(StoreStage::Query, path))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| store_error(StoreStage::Query, path))?
        .into_iter()
        .filter_map(|body| serde_json::from_str::<tela_proto::FeedRow>(&body).ok())
        .filter(|feed| feed.site_id == site_id)
        .map(|feed| feed.id)
        .collect::<Vec<_>>();
    drop(stmt);

    let mut affected = 0usize;
    for feed_id in feed_ids {
        let mut stmt = tx
            .prepare("SELECT body FROM sync_rows WHERE entity = 'articles'")
            .map_err(|_| store_error(StoreStage::Query, path))?;
        let articles = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| store_error(StoreStage::Query, path))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| store_error(StoreStage::Query, path))?
            .into_iter()
            .filter_map(|body| serde_json::from_str::<tela_proto::ArticleRow>(&body).ok())
            .filter(|article| article.feed_id == feed_id)
            .collect::<Vec<_>>();
        drop(stmt);

        for article in articles {
            affected += tx
                .execute(
                    "DELETE FROM sync_rows WHERE entity = 'titles' AND row_key LIKE ?1",
                    params![format!("{}/%", article.id)],
                )
                .map_err(|_| store_error(StoreStage::Query, path))?;
            if let Some(content_key) = article.content_key {
                affected += tx
                    .execute(
                        "DELETE FROM sync_rows WHERE entity = 'translations' AND row_key LIKE ?1",
                        params![format!("{content_key}/%")],
                    )
                    .map_err(|_| store_error(StoreStage::Query, path))?;
            }
        }
    }
    Ok(affected)
}

/// 资料行的固定主键（供测试/诊断）。
pub const PROFILE_ROW_KEY: &str = ProfileRow::ROW_KEY;
