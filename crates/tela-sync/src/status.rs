//! 同步状态快照：游标、行数、待发队列、最近错误。只读本地库，不联网。

use tela_proto::{DEFAULT_SYNC_TYPES, TelaError};
use tela_store::Store;

/// 一次状态查询的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncStatus {
    /// 已登录成员 handle。
    pub account: Option<String>,
    /// 同步游标。
    pub cursor: Option<String>,
    /// 游标对应的语言。
    pub lang: Option<String>,
    /// 游标覆盖的行类型。
    pub types: Vec<String>,
    /// 各表的行数（按固定表序）。
    pub row_counts: Vec<(&'static str, usize)>,
    /// 待发变更数。
    pub pending: usize,
    /// 缓存的正文对象数。
    pub bodies: usize,
    /// 最近一次失败文案。
    pub last_error: Option<String>,
}

impl SyncStatus {
    /// 读本地库生成快照。
    pub fn snapshot(store: &Store, last_error: Option<String>) -> Result<Self, TelaError> {
        let mut row_counts = Vec::with_capacity(DEFAULT_SYNC_TYPES.len());
        for table in DEFAULT_SYNC_TYPES {
            row_counts.push((table, store.row_count(table)?));
        }
        Ok(Self {
            account: store.account()?,
            cursor: store.cursor()?,
            lang: store.sync_lang()?,
            types: store.sync_types()?,
            row_counts,
            pending: store.pending_count()?,
            bodies: store.body_count()?,
            last_error,
        })
    }

    /// 全部同步行的总数。
    pub fn total_rows(&self) -> usize {
        self.row_counts.iter().map(|(_, count)| count).sum()
    }

    /// 给 CLI 的多行文本。
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "account  {}\n",
            self.account.as_deref().unwrap_or("(未登录)")
        ));
        out.push_str(&format!(
            "cursor   {}\n",
            self.cursor.as_deref().unwrap_or("(从未同步)")
        ));
        out.push_str(&format!(
            "lang     {}\n",
            self.lang.as_deref().unwrap_or("-")
        ));
        out.push_str(&format!(
            "rows     {} / pending {}",
            self.total_rows(),
            self.pending
        ));
        out.push_str(&format!("  bodies {}\n", self.bodies));
        for (table, count) in &self.row_counts {
            if *count > 0 {
                out.push_str(&format!("  {table:<16} {count}\n"));
            }
        }
        if let Some(error) = &self.last_error {
            out.push_str(&format!("last     {error}\n"));
        }
        out
    }
}
