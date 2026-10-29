//! 会话：访问令牌的内存缓存、刷新与登出。
//!
//! 约定（`docs/api/authorization.md` §「In an app」）：
//!
//! - 访问令牌只留在内存，带过期时间；距过期不足一分钟就先刷新；
//! - 刷新令牌每次使用都换新，**先落存储再用新令牌**；
//! - 同一设备同一时刻只允许一个刷新在飞（用互斥锁串行化）；
//! - 网络失败时保留旧刷新令牌，两分钟内可用同一个重试；
//! - `invalid_grant` 表示连接/设备已结束，只能重新登录；`unauthorized_client`
//!   表示应用被临时停用，令牌要保留。

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tela_proto::{TelaError, WireContext};

use crate::callback::{CallbackAnswer, PendingLogin};
use crate::oauth::{AuthConfig, Tokens, refresh_tokens, revoke_token};
use crate::token_store::{StoredCredential, TokenStore};

/// 访问令牌距过期不足这个余量时先刷新。
pub const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// 任何能给 API 客户端提供 Bearer 令牌的东西。
pub trait TokenProvider: Send + Sync {
    /// 取一个可用的访问令牌（必要时刷新）。
    fn access_token(&self) -> Result<String, TelaError>;
    /// 强制刷新（HTTP 401 后调用一次）。
    fn refresh(&self) -> Result<(), TelaError>;
}

/// 固定令牌（测试与本地 mock 用）。
#[derive(Debug, Clone)]
pub struct StaticToken(pub String);

impl TokenProvider for StaticToken {
    fn access_token(&self) -> Result<String, TelaError> {
        Ok(self.0.clone())
    }

    fn refresh(&self) -> Result<(), TelaError> {
        Err(TelaError::NotLoggedIn {
            detail: "测试用静态令牌无法刷新".into(),
        })
    }
}

#[derive(Debug, Default)]
struct SessionState {
    tokens: Option<Tokens>,
    scope: Option<String>,
    handle: Option<String>,
    connected: bool,
    /// 打开凭据库时的错误（例如没有 Secret Service）；登录时会再报一次。
    store_error: Option<String>,
}

/// 一次登录的结果，供 CLI 展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginOutcome {
    /// 成员授予的 scope。
    pub scope: String,
    /// 访问令牌过期时间。
    pub expires_at_ms: i64,
}

/// 已登录（或曾经登录）的会话。
pub struct AuthSession {
    config: AuthConfig,
    store: Arc<dyn TokenStore>,
    account: String,
    state: Mutex<SessionState>,
    refresh_lock: Mutex<()>,
}

impl AuthSession {
    /// 从存储加载凭据；没有凭据也可以构造（`access_token` 会报未登录）。
    pub fn load(config: AuthConfig, store: Arc<dyn TokenStore>) -> Result<Self, TelaError> {
        let account = "default".to_string();
        // 读不到凭据库不等于程序不能用：list/status/meta 都可以离线工作，
        // 只有真正登录/刷新时才必须有一个可写的凭据库。
        let (credential, store_error) = match store.load(&account) {
            Ok(credential) => (credential, None),
            Err(error) => (None, Some(error.to_string())),
        };
        let state = SessionState {
            tokens: None,
            connected: credential.is_some(),
            scope: credential.as_ref().map(|item| item.scope.clone()),
            handle: credential.and_then(|item| item.handle),
            store_error,
        };
        Ok(Self {
            config,
            store,
            account,
            state: Mutex::new(state),
            refresh_lock: Mutex::new(()),
        })
    }

    /// 是否已经有刷新令牌（不代表一定还能用）。
    pub fn is_connected(&self) -> bool {
        self.lock_state().connected
    }

    /// 配置的 public client id。
    pub fn client_id(&self) -> &str {
        &self.config.client_id
    }

    /// 打开凭据库时的错误（没有 Secret Service、权限不足等）。
    pub fn store_error(&self) -> Option<String> {
        self.lock_state().store_error.clone()
    }

    /// 已知的成员 handle。
    pub fn handle(&self) -> Option<String> {
        self.lock_state().handle.clone()
    }

    /// 记录成员 handle（`/v1/me` 之后调用，写回凭据库）。
    pub fn set_handle(&self, handle: &str) -> Result<(), TelaError> {
        let mut state = self.lock_state();
        state.handle = Some(handle.to_string());
        if let Some(tokens) = &state.tokens {
            let credential = StoredCredential {
                refresh_token: tokens.refresh_token.clone(),
                scope: tokens.scope.clone(),
                handle: Some(handle.to_string()),
            };
            self.store.save(&self.account, &credential)?;
            return Ok(());
        }
        drop(state);
        if let Some(mut credential) = self.store.load(&self.account)? {
            credential.handle = Some(handle.to_string());
            self.store.save(&self.account, &credential)?;
        }
        Ok(())
    }

    /// 当前 scope（成员实际授予的）。
    pub fn scope(&self) -> Option<String> {
        self.lock_state().scope.clone()
    }

    /// 当前访问令牌（未过期且在内存里时）。
    pub fn current_access_token(&self) -> Option<String> {
        self.lock_state()
            .tokens
            .as_ref()
            .map(|tokens| tokens.access_token.clone())
    }

    /// 交互式登录：开端口、拼 URL、等回调、换令牌、落刷新令牌。
    ///
    /// `on_url` 在浏览器打开之前调用，CLI 用它把地址打印出来（打不开浏览器时也能手动访问）。
    pub fn login<F>(
        &self,
        open_browser: bool,
        timeout: Duration,
        on_url: F,
    ) -> Result<LoginOutcome, TelaError>
    where
        F: FnOnce(&str),
    {
        let pending = self.begin_login()?;
        on_url(pending.auth_url());

        if open_browser {
            crate::browser::open(pending.auth_url())?;
        }

        self.finish_login(pending, timeout)
    }

    /// 第一步：校验配置/凭据库，绑定回环端口并生成授权 URL。
    ///
    /// GUI 需要先把 URL 展示出来，再把等待放到后台，所以把登录拆成两段。
    pub fn begin_login(&self) -> Result<PendingLogin, TelaError> {
        self.config.validate()?;
        if let Some(error) = self.store_error() {
            return Err(TelaError::Config {
                detail: format!("{error}；修复凭据库后重试，或临时使用 `--ephemeral`"),
            });
        }
        PendingLogin::begin(&self.config)
    }

    /// 第二步：阻塞等待浏览器回调，换令牌并更新内存状态。
    ///
    /// `PendingLogin` 被消费掉；失败后这张票据作废，需要重新 `begin_login`。
    pub fn finish_login(
        &self,
        pending: PendingLogin,
        timeout: Duration,
    ) -> Result<LoginOutcome, TelaError> {
        let answer: CallbackAnswer = pending.wait(timeout)?;
        let tokens = pending.complete(&answer, self.store.as_ref(), &self.account)?;
        let outcome = LoginOutcome {
            scope: tokens.scope.clone(),
            expires_at_ms: tokens.expires_at_ms,
        };
        let mut state = self.lock_state();
        state.scope = Some(tokens.scope.clone());
        state.tokens = Some(tokens);
        state.connected = true;
        Ok(outcome)
    }

    /// 用刷新令牌换一对新令牌；成功后才覆盖存储里的旧令牌。
    pub fn refresh(&self) -> Result<(), TelaError> {
        let _guard = self.lock_refresh();
        {
            // 另一个线程可能刚刷新过：令牌仍然有效就直接复用。
            let state = self.lock_state();
            if let Some(tokens) = &state.tokens
                && !tokens.is_expiring(tela_proto::now_ms(), REFRESH_MARGIN)
            {
                return Ok(());
            }
        }

        let credential = self
            .store
            .load(&self.account)?
            .ok_or_else(|| TelaError::NotLoggedIn {
                detail: "本地没有刷新令牌".into(),
            })?;

        let tokens = refresh_tokens(&self.config, &credential.refresh_token)?;
        // 先落存储，再用新令牌：旧刷新令牌已经被消费。
        self.store.save(
            &self.account,
            &StoredCredential {
                refresh_token: tokens.refresh_token.clone(),
                scope: tokens.scope.clone(),
                handle: self.lock_state().handle.clone(),
            },
        )?;

        let mut state = self.lock_state();
        state.scope = Some(tokens.scope.clone());
        state.tokens = Some(tokens);
        Ok(())
    }

    /// 登出：撤销该设备令牌并删除本地凭据。
    ///
    /// 网络失败也会清掉本地凭据（用户明确要登出），但会把错误返回给调用方提示。
    pub fn logout(&self) -> Result<(), TelaError> {
        let refresh_token = match self.lock_state().tokens.as_ref() {
            Some(tokens) => Some(tokens.refresh_token.clone()),
            None => self
                .store
                .load(&self.account)?
                .map(|credential| credential.refresh_token),
        };

        let mut revoke_error = None;
        if let Some(token) = refresh_token
            && let Err(err) = revoke_token(&self.config, &token)
        {
            revoke_error = Some(err);
        }
        self.store.delete(&self.account)?;
        {
            let mut state = self.lock_state();
            state.tokens = None;
            state.scope = None;
            state.handle = None;
            state.connected = false;
        }
        match revoke_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, SessionState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn lock_refresh(&self) -> MutexGuard<'_, ()> {
        match self.refresh_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl TokenProvider for AuthSession {
    fn access_token(&self) -> Result<String, TelaError> {
        {
            let state = self.lock_state();
            if let Some(tokens) = &state.tokens
                && !tokens.is_expiring(tela_proto::now_ms(), REFRESH_MARGIN)
            {
                return Ok(tokens.access_token.clone());
            }
        }
        if !self.is_connected() {
            return Err(TelaError::NotLoggedIn {
                detail: "本地没有可用的 Tela 凭据".into(),
            });
        }
        self.refresh()?;
        self.lock_state()
            .tokens
            .as_ref()
            .map(|tokens| tokens.access_token.clone())
            .ok_or_else(|| TelaError::NotLoggedIn {
                detail: "刷新后仍没有访问令牌".into(),
            })
    }

    fn refresh(&self) -> Result<(), TelaError> {
        AuthSession::refresh(self)
    }
}

/// 把「刷新失败」翻译成给用户看的下一步（CLI 用）。
pub fn explain_refresh_error(error: &TelaError) -> String {
    match error {
        TelaError::Auth { detail, .. } if detail.contains("invalid_grant") => {
            format!("{detail}；连接或设备已结束，请重新运行 `tela-app login`")
        }
        TelaError::Auth { detail, .. } if detail.contains("unauthorized_client") => {
            format!("{detail}；应用被临时停用，令牌已保留，恢复后会自动可用")
        }
        _ => error.to_string(),
    }
}

/// 构造一个带 URL 的上下文（测试/日志用）。
pub fn token_context(config: &AuthConfig) -> WireContext {
    WireContext::new(config.token_endpoint(), "oauth/token")
}
