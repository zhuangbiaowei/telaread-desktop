//! `/v1/sync` 的响应（`docs/api/v1.md` § Sync、OpenAPI `Sync`）。
//!
//! 一页响应回答四件事：从哪继续（`cursor`）、还有没有（`more`）、是不是快照
//! （`reset`）、以及这一页的行与删除清单。

use serde::{Deserialize, Serialize};

use crate::error::{ProtocolStage, TelaError};
use crate::rows::{Removed, SyncRows};

/// 一次 pull 的响应页。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResponse {
    /// 不透明游标：与行一起持久化，下次原样送回。
    pub cursor: String,
    /// 是否还有下一页（立刻用新 cursor 续拉）。
    #[serde(default)]
    pub more: bool,
    /// 是否是快照：`true` 时先丢掉该账号的全部行，再接收本页。
    #[serde(default)]
    pub reset: bool,
    /// 本次译文的语言。
    pub lang: String,
    /// 本次覆盖的行类型。
    #[serde(default)]
    pub types: Vec<String>,
    /// 要从设备上删除的行。
    #[serde(default)]
    pub removed: Removed,
    /// 行数据。
    pub rows: SyncRows,
}

impl SyncResponse {
    /// 从 wire JSON 解码整页，并把 serde 的英文错误折算成可定位的字段路径。
    pub fn decode(json: &str) -> Result<Self, TelaError> {
        serde_json::from_str(json).map_err(|err| TelaError::Protocol {
            stage: ProtocolStage::ShapeMismatch,
            object: shape_object(&err),
        })
    }

    /// 本页携带的行数。
    pub fn row_count(&self) -> usize {
        self.rows.row_count() + self.removed.articles.len()
    }
}

/// `serde_json` 的 `missing field \`x\`` / `unknown field \`x\`` → `x`。
fn shape_object(err: &serde_json::Error) -> String {
    let text = err.to_string();
    for marker in ["missing field `", "unknown field `"] {
        let name = text
            .split(marker)
            .nth(1)
            .and_then(|rest| rest.split('`').next())
            .filter(|name| !name.is_empty());
        if let Some(name) = name {
            return name.to_string();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_response_decodes_with_defaults() {
        let json = r#"{"cursor":"c1","lang":"en","rows":{}}"#;
        let page = SyncResponse::decode(json).unwrap();
        assert_eq!(page.cursor, "c1");
        assert!(!page.more);
        assert!(!page.reset);
        assert!(page.removed.articles.is_empty());
        assert_eq!(page.rows.row_count(), 0);
    }

    #[test]
    fn cursor_is_required_and_reported_by_name() {
        let err = SyncResponse::decode(r#"{"lang":"en","rows":{}}"#).unwrap_err();
        assert!(err.to_string().contains("cursor"), "{err}");
    }
}
