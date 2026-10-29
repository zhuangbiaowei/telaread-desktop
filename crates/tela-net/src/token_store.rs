//! 刷新令牌的持久化。
//!
//! 文档要求（`docs/api/authorization.md`「Keeping tokens」）：刷新令牌只能放操作系统的
//! 凭据库——macOS Keychain、Windows Credential Manager、Linux Secret Service——
//! **绝不能**写普通文件、日志、崩溃报告或 URL。
//!
//! 因此默认实现是 [`KeyringTokenStore`]；[`MemoryTokenStore`] 只用于测试和显式
//! `--ephemeral` 的临时会话（进程退出即丢，不落盘）。

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tela_proto::TelaError;

/// keyring 里保存的凭据内容。
///
/// `refresh_token` 是唯一长期秘密；`handle` 与 `scope` 只用于展示和判断是否需要重新授权。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCredential {
    /// 刷新令牌（`tela_rt_…`）。
    pub refresh_token: String,
    /// 授权时拿到的 scope（成员可能把 write 降成 read）。
    #[serde(default)]
    pub scope: String,
    /// 成员 handle（便于 `status` 展示）。
    #[serde(default)]
    pub handle: Option<String>,
}

/// 令牌存储抽象。
pub trait TokenStore: Send + Sync {
    /// 读取该账号的凭据；没有时返回 `Ok(None)`。
    fn load(&self, account: &str) -> Result<Option<StoredCredential>, TelaError>;
    /// 写入/覆盖。
    fn save(&self, account: &str, credential: &StoredCredential) -> Result<(), TelaError>;
    /// 删除；不存在也算成功。
    fn delete(&self, account: &str) -> Result<(), TelaError>;
}

/// keyring 的 service 名（固定，升级不换，否则老用户会「掉登录」）。
pub const KEYRING_SERVICE: &str = "com.telaread.desktop";

/// 系统凭据库实现。
#[derive(Debug, Clone)]
pub struct KeyringTokenStore {
    service: String,
}

impl Default for KeyringTokenStore {
    fn default() -> Self {
        Self::new(KEYRING_SERVICE)
    }
}

impl KeyringTokenStore {
    /// 指定 service 名（测试用）。
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, account: &str) -> Result<keyring::Entry, TelaError> {
        keyring::Entry::new(&self.service, account).map_err(|err| TelaError::Config {
            detail: format!(
                "无法访问系统凭据库（{}）：{err}；\
                 无桌面密钥环时可显式使用 `--ephemeral` 做临时会话",
                self.service
            ),
        })
    }
}

impl TokenStore for KeyringTokenStore {
    fn load(&self, account: &str) -> Result<Option<StoredCredential>, TelaError> {
        let entry = self.entry(account)?;
        match entry.get_password() {
            Ok(secret) => {
                serde_json::from_str(&secret)
                    .map(Some)
                    .map_err(|err| TelaError::Config {
                        detail: format!("凭据库里的记录无法解析：{err}；重新登录可覆盖它"),
                    })
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(TelaError::Config {
                detail: format!("读取系统凭据失败：{err}"),
            }),
        }
    }

    fn save(&self, account: &str, credential: &StoredCredential) -> Result<(), TelaError> {
        let secret = serde_json::to_string(credential).map_err(|err| TelaError::Config {
            detail: format!("凭据无法序列化：{err}"),
        })?;
        self.entry(account)?
            .set_password(&secret)
            .map_err(|err| TelaError::Config {
                detail: format!("写入系统凭据失败：{err}"),
            })
    }

    fn delete(&self, account: &str) -> Result<(), TelaError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(TelaError::Config {
                detail: format!("删除系统凭据失败：{err}"),
            }),
        }
    }
}

/// 进程内实现：测试与 `--ephemeral` 用，**不落盘**。
#[derive(Debug, Default)]
pub struct MemoryTokenStore {
    inner: Mutex<HashMap<String, StoredCredential>>,
}

impl MemoryTokenStore {
    /// 建一个空存储。
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, StoredCredential>> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl TokenStore for MemoryTokenStore {
    fn load(&self, account: &str) -> Result<Option<StoredCredential>, TelaError> {
        Ok(self.lock().get(account).cloned())
    }

    fn save(&self, account: &str, credential: &StoredCredential) -> Result<(), TelaError> {
        self.lock().insert(account.to_string(), credential.clone());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), TelaError> {
        self.lock().remove(account);
        Ok(())
    }
}
