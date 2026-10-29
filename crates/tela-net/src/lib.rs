//! `tela-net` — 真实 Tela API 的传输与认证层。
//!
//! 这一层只做「把请求安全地送出去、把响应解回来」：
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`oauth`] | OAuth 2.1 配置、PKCE（S256）、授权 URL、换令牌/刷新/撤销 |
//! | [`callback`] | 回环 `127.0.0.1:<随机端口>` 监听与 `state`/`iss` 校验 |
//! | [`browser`] | 打开系统浏览器（绝不用应用内 WebView） |
//! | [`session`] | 访问令牌内存缓存、单飞刷新、登出 |
//! | [`token_store`] | 刷新令牌的系统凭据库持久化（Keychain / Credential Manager / Secret Service） |
//! | [`client`] | Bearer 认证、JSON、错误映射、401 自动刷新一次 |
//! | [`transport`] | `tela-sync` 认识的 `pull` / `push` 抽象 + 内存 mock |
//!
//! 安全约束（docs/api/authorization.md）：
//! - 访问令牌只放 `Authorization` 头，绝不进 query；
//! - 刷新令牌只进系统凭据库，绝不写普通文件；
//! - 令牌不写日志、不进崩溃报告。

pub mod browser;
pub mod callback;
pub mod client;
pub mod oauth;
pub mod session;
pub mod token_store;
pub mod transport;

pub use client::{ApiClient, DEFAULT_TIMEOUT};
pub use oauth::{
    AuthConfig, Pkce, Tokens, authorization_url, exchange_code, refresh_tokens, revoke_token,
};
pub use session::{AuthSession, LoginOutcome, StaticToken, TokenProvider};
pub use token_store::{KeyringTokenStore, MemoryTokenStore, StoredCredential, TokenStore};
pub use transport::{MockTransport, SyncQuery, SyncTransport, TransportCall};
