//! 应用装配层：`Store` + `ContentApi` + `SyncTransport` + 认证会话。
//!
//! 三条装配路径：
//!
//! | 入口 | 用途 | 数据 |
//! |---|---|---|
//! | [`AppServices::bootstrap_real`] | CLI / 真实 GUI | 真实 API（keyring 里的刷新令牌） |
//! | [`AppServices::bootstrap_demo`] | GUI 演示、离线开发、CI | 进程内 mock + 演示 seed |
//! | [`AppServices::open`] / [`AppServices::open_in_memory`] | 测试 | 调用方注入 |
//!
//! 本层是唯一同时接触 `Store`、`ContentApi`、`SyncTransport`、`AuthSession` 的地方；
//! UI 只拿到只读的 [`ShellData`] 快照与 `Arc<dyn ContentApi>`。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tela_content::api::{ChapterRequest, ContentApi, http_content_api, mock_content_api};
use tela_content::http::normalize_content_key;
use tela_content::mock::{
    MOCK_CONTENT_KEY, MOCK_LONG_CONTENT_KEY, MOCK_NO_TRANSLATION_CONTENT_KEY,
};
use tela_content::model::{Chapter, ContentObject};
use tela_net::callback::PendingLogin;
use tela_net::{
    ApiClient, AuthConfig, AuthSession, KeyringTokenStore, MemoryTokenStore, MockTransport,
    SyncTransport, TokenProvider, TokenStore,
};
use tela_proto::{
    ArticleRow, DEFAULT_HORIZON_DAYS, DEFAULT_SYNC_TYPES, FeedRow, Me, Meta, PrefRow,
    ReadRuleInput, Removed, SiteRow, StateRow, StoreStage, SubscriptionRow, SyncResponse, SyncRows,
    TelaError, TitleRow, Usage, is_read, now_ms,
};
use tela_store::Store;
use tela_sync::{PullError, PullOptions, SyncEngine, SyncReport, SyncStatus};
use tela_ui::account::{AccountMode, AccountService, AccountStatus, LoginTicket};
use tela_ui::shell::{ArticleItem, FeedItem, ShellData};

/// 数据库路径覆盖开关。
pub const ENV_DB: &str = "TELA_DB";
/// 译文语言覆盖开关。
pub const ENV_LANG: &str = "TELA_LANG";
/// 演示数据开关：`0` 关闭；demo 装配默认开启。
pub const ENV_DEV_SEED: &str = "TELA_DEV_SEED";
/// API 基址。
pub const ENV_API_BASE_URL: &str = "TELA_API_BASE_URL";
/// 授权页站点。
pub const ENV_AUTH_BASE_URL: &str = "TELA_AUTH_BASE_URL";
/// 注册得到的 public client id（`tela_app_…`）。
pub const ENV_CLIENT_ID: &str = "TELA_CLIENT_ID";
/// scope 覆盖（默认 `write`，需要邮箱时写 `write email`）。
pub const ENV_SCOPES: &str = "TELA_SCOPES";
/// 令牌存储：`keyring`（默认）或 `memory`（临时会话）。
pub const ENV_TOKEN_STORE: &str = "TELA_TOKEN_STORE";
/// GUI 是否使用真实 API：默认真实（登录模式）；`1`/`true`/`yes`/`on` = demo。
pub const ENV_DEMO: &str = "TELA_DEMO";

/// 发布包内置的 public client id。
///
/// 构建时由同名环境变量注入（`release.yml` 用仓库变量 `vars.TELA_CLIENT_ID`），
/// 用户解压后直接就是登录模式，不需要自己注册/配置 client id。
/// public client id 不是秘密；运行时的 `TELA_CLIENT_ID` 仍然优先，便于本地调试。
pub const DEFAULT_CLIENT_ID: &str = match option_env!("TELA_CLIENT_ID") {
    Some(value) => value,
    None => "",
};

/// 默认译文语言（与 mock fixture 对齐）。
pub const DEFAULT_LANG: &str = "zh-Hans";

/// 演示数据的账号标记：真实登录打开同一个库时必须先清掉它。
pub const DEMO_ACCOUNT: &str = "demo";

/// 演示订阅 1。
pub const DEMO_FEED_ID: i64 = 1;
/// 演示订阅 2。
pub const DEMO_FEED_ID_2: i64 = 2;
/// 演示文章：正常章节。
pub const DEMO_ARTICLE_NORMAL: i64 = 42;
/// 演示文章：长章节。
pub const DEMO_ARTICLE_LONG: i64 = 43;
/// 演示文章：无译文。
pub const DEMO_ARTICLE_NO_TRANSLATION: i64 = 44;
/// 演示文章：已读。
pub const DEMO_ARTICLE_READ: i64 = 45;

/// 令牌存储选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenBackend {
    /// 系统凭据库。
    Keyring,
    /// 进程内（不落盘），用于测试与 `--ephemeral`。
    Memory,
}

/// 应用配置：只放「怎么打开」，不放业务状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    /// SQLite 路径。
    pub db_path: PathBuf,
    /// 译文语言。
    pub lang: String,
    /// 空库时是否写演示数据。
    pub seed_demo: bool,
    /// API 基址。
    pub api_base: String,
    /// 授权页站点。
    pub auth_base: String,
    /// public client id。
    pub client_id: String,
    /// scope。
    pub scopes: String,
    /// 令牌存储。
    pub token_backend: TokenBackend,
    /// HTTP 超时。
    pub timeout: Duration,
    /// 阅读地平线（天）。
    pub horizon_days: i64,
}

impl AppConfig {
    /// 从环境变量装配。
    pub fn from_env() -> Self {
        let lang = env_string(ENV_LANG)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_LANG.to_string());
        let seed_demo = match env_string(ENV_DEV_SEED).as_deref() {
            Some("0") => false,
            Some(_) => true,
            None => false,
        };
        let token_backend = match env_string(ENV_TOKEN_STORE)
            .map(|value| value.to_ascii_lowercase())
            .as_deref()
        {
            Some("memory") | Some("ephemeral") => TokenBackend::Memory,
            _ => TokenBackend::Keyring,
        };
        Self {
            db_path: db_path_from_env(),
            lang,
            seed_demo,
            api_base: env_string(ENV_API_BASE_URL)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| tela_proto::DEFAULT_API_BASE_URL.to_string()),
            auth_base: env_string(ENV_AUTH_BASE_URL)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| tela_proto::DEFAULT_AUTH_BASE_URL.to_string()),
            client_id: env_string(ENV_CLIENT_ID)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| DEFAULT_CLIENT_ID.trim().to_string()),
            scopes: env_string(ENV_SCOPES)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "write".to_string()),
            token_backend,
            timeout: Duration::from_secs(30),
            horizon_days: DEFAULT_HORIZON_DAYS,
        }
    }

    /// 测试配置（不 seed、默认地址）。
    pub fn for_tests(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
            lang: DEFAULT_LANG.to_string(),
            seed_demo: false,
            api_base: tela_proto::DEFAULT_API_BASE_URL.to_string(),
            auth_base: tela_proto::DEFAULT_AUTH_BASE_URL.to_string(),
            client_id: "tela_app_test".to_string(),
            scopes: "write".to_string(),
            token_backend: TokenBackend::Memory,
            timeout: Duration::from_secs(10),
            horizon_days: DEFAULT_HORIZON_DAYS,
        }
    }

    /// 认证配置。
    pub fn auth_config(&self) -> AuthConfig {
        let mut config = AuthConfig::new(self.client_id.clone());
        config.api_base = self.api_base.clone();
        config.auth_base = self.auth_base.clone();
        config.scopes = self.scopes.clone();
        config.timeout = self.timeout;
        config
    }
}

fn env_string(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
}

/// 默认库路径。
///
/// `TELA_DB` > `$XDG_DATA_HOME/tela/tela.db` > Windows 的
/// `%APPDATA%\tela\tela.db` > `$HOME/.local/share/tela/tela.db`。
///
/// Windows 上通常没有 `HOME`/`XDG_DATA_HOME`，必须走 `%APPDATA%`，否则会退回
/// 「当前目录的 tela.db」——双击 exe 时就是解压目录，既不合规范也容易被误删。
fn db_path_from_env() -> PathBuf {
    if let Some(raw) = std::env::var_os(ENV_DB) {
        return PathBuf::from(raw);
    }
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(dir).join("tela").join("tela.db");
    }
    #[cfg(windows)]
    {
        for key in ["APPDATA", "LOCALAPPDATA", "USERPROFILE"] {
            if let Some(dir) = std::env::var_os(key) {
                return PathBuf::from(dir).join("tela").join("tela.db");
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("tela")
            .join("tela.db");
    }
    PathBuf::from("tela.db")
}

/// 应用服务。
pub struct AppServices {
    store: Arc<Mutex<Store>>,
    content: Arc<dyn ContentApi>,
    transport: Arc<dyn SyncTransport>,
    auth: Option<Arc<AuthSession>>,
    api: Option<Arc<ApiClient>>,
    lang: String,
    db_path: PathBuf,
    horizon_days: i64,
    /// GUI 登录中尚未等到的回环监听（nonce + pending）。
    pending_login: Mutex<Option<(u64, PendingLogin)>>,
    next_login: AtomicU64,
}

impl AppServices {
    /// 低层装配：调用方自己给内容后端与传输层（测试/嵌入用）。
    pub fn open(
        config: AppConfig,
        content: Arc<dyn ContentApi>,
        transport: Arc<dyn SyncTransport>,
    ) -> Result<Self, TelaError> {
        if let Some(parent) = config.db_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|_| TelaError::Store {
                stage: StoreStage::Open,
                path: config.db_path.display().to_string(),
            })?;
        }
        let store = Store::open(&config.db_path)?;
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
            content,
            transport,
            auth: None,
            api: None,
            lang: config.lang,
            db_path: config.db_path,
            horizon_days: config.horizon_days,
            pending_login: Mutex::new(None),
            next_login: AtomicU64::new(1),
        })
    }

    /// 内存库（测试）。
    pub fn open_in_memory(
        lang: impl Into<String>,
        content: Arc<dyn ContentApi>,
        transport: Arc<dyn SyncTransport>,
    ) -> Result<Self, TelaError> {
        Ok(Self {
            store: Arc::new(Mutex::new(Store::open_in_memory()?)),
            content,
            transport,
            auth: None,
            api: None,
            lang: lang.into(),
            db_path: PathBuf::from(":memory:"),
            horizon_days: DEFAULT_HORIZON_DAYS,
            pending_login: Mutex::new(None),
            next_login: AtomicU64::new(1),
        })
    }

    /// 真实 API 装配（CLI 与真实 GUI）。
    pub fn bootstrap_real() -> Result<Self, TelaError> {
        Self::bootstrap_real_with(AppConfig::from_env())
    }

    /// 用显式配置装配真实 API（CLI 的 `--ephemeral` 等开关用）。
    pub fn bootstrap_real_with(config: AppConfig) -> Result<Self, TelaError> {
        let mut store = open_store(&config.db_path)?;
        let token_store: Arc<dyn TokenStore> = match config.token_backend {
            TokenBackend::Keyring => Arc::new(KeyringTokenStore::default()),
            TokenBackend::Memory => Arc::new(MemoryTokenStore::new()),
        };
        let auth = Arc::new(AuthSession::load(config.auth_config(), token_store)?);
        let provider: Arc<dyn TokenProvider> = auth.clone();
        let api = Arc::new(ApiClient::with_timeout(
            config.api_base.clone(),
            provider,
            config.timeout,
        )?);
        let content = http_content_api(api.clone());
        let transport: Arc<dyn SyncTransport> = api.clone();

        // 同一个 SQLite 文件可能同时被 demo 与真实装配打开。demo 行不能带进真实账号：
        // - 旧 demo 库没有账号标记，但游标固定是 `demo-snapshot`；
        // - 新 demo 库账号标记是 `demo`；
        // - 已登录账号与库里的账号不同，同样要清（登录切换时也会清）。
        let stored_account = store.account()?;
        let demo_db = stored_account.as_deref() == Some(DEMO_ACCOUNT)
            || store.cursor()?.as_deref() == Some("demo-snapshot");
        let mismatch = matches!(
            (stored_account.as_deref(), auth.handle().as_deref()),
            (Some(stored), Some(logged_in)) if stored != logged_in && stored != DEMO_ACCOUNT
        );
        if demo_db || mismatch {
            store.clear_account()?;
        }
        if let Some(handle) = auth.handle() {
            store.set_account(&handle)?;
        }

        let services = Self {
            store: Arc::new(Mutex::new(store)),
            content,
            transport,
            auth: Some(auth),
            api: Some(api),
            lang: config.lang,
            db_path: config.db_path,
            horizon_days: config.horizon_days,
            pending_login: Mutex::new(None),
            next_login: AtomicU64::new(1),
        };
        // 真实装配绝不写演示数据：`TELA_DEV_SEED` 只对 demo 装配生效。
        let _ = config.seed_demo;
        Ok(services)
    }

    /// 演示装配：mock 内容 + 不联网传输 + 空库 seed。
    pub fn bootstrap_demo() -> Result<Self, TelaError> {
        let config = AppConfig::from_env();
        let content: Arc<dyn ContentApi> = Arc::from(mock_content_api());
        let transport: Arc<dyn SyncTransport> = Arc::new(MockTransport::new());
        let services = Self::open(config.clone(), content, transport)?;
        if config.seed_demo || demo_is_default() {
            services.seed_demo()?;
        }
        Ok(services)
    }

    /// GUI 装配：默认真实 API（登录模式），`--demo` 或 `TELA_DEMO=1` 进入演示。
    ///
    /// demo 是显式选择：发布包内置 client id（见 [`DEFAULT_CLIENT_ID`]），
    /// 用户双击就应该看到「登录 Tela」，而不是只有演示数据。
    pub fn bootstrap_gui(force_demo: bool) -> Result<Self, TelaError> {
        if force_demo || demo_requested_by_env() {
            Self::bootstrap_demo()
        } else {
            Self::bootstrap_real()
        }
    }

    /// 库路径。
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// 目标译文语言。
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// 阅读地平线。
    pub fn horizon_days(&self) -> i64 {
        self.horizon_days
    }

    /// 内容 API 句柄。
    pub fn content(&self) -> Arc<dyn ContentApi> {
        Arc::clone(&self.content)
    }

    /// 认证会话（demo 装配没有）。
    pub fn auth(&self) -> Option<&Arc<AuthSession>> {
        self.auth.as_ref()
    }

    /// API 客户端（demo 装配没有）。
    pub fn api(&self) -> Option<&Arc<ApiClient>> {
        self.api.as_ref()
    }

    /// 是否已登录（有刷新令牌）。
    pub fn is_connected(&self) -> bool {
        self.auth.as_ref().is_some_and(|auth| auth.is_connected())
    }

    /// 配置里的 public client id（未配置时为空串）。
    pub fn client_id(&self) -> Option<&str> {
        self.auth.as_ref().map(|auth| auth.client_id())
    }

    /// 凭据库不可用时的原因（密钥环未解锁、没有 Secret Service 等）。
    pub fn credential_error(&self) -> Option<String> {
        self.auth.as_ref().and_then(|auth| auth.store_error())
    }

    // ---- 认证 ----

    /// 交互式登录，成功后调用 `/v1/me` 记录 handle。
    pub fn login<F>(
        &self,
        open_browser: bool,
        timeout: Duration,
        on_url: F,
    ) -> Result<Me, TelaError>
    where
        F: FnOnce(&str),
    {
        let auth = self.auth.as_ref().ok_or_else(|| TelaError::Config {
            detail: "demo 装配不支持登录；用 `tela-app login`（真实装配）".into(),
        })?;
        let api = self.api.as_ref().ok_or_else(|| TelaError::Config {
            detail: "缺少 API 客户端".into(),
        })?;
        auth.login(open_browser, timeout, on_url)?;
        let me = api.me()?;
        self.settle_login(&me)?;
        Ok(me)
    }

    /// 登出：撤销令牌、清本地账号数据（保留正文缓存）。
    pub fn logout(&self) -> Result<(), TelaError> {
        let auth = self.auth.as_ref().ok_or_else(|| TelaError::Config {
            detail: "demo 装配没有登录状态".into(),
        })?;
        let result = auth.logout();
        self.lock_store()?.clear_account()?;
        result
    }

    /// `/v1/me`。
    pub fn me(&self) -> Result<Me, TelaError> {
        self.api
            .as_ref()
            .ok_or_else(|| TelaError::NotLoggedIn {
                detail: "demo 装配没有账号".into(),
            })?
            .me()
    }

    /// `/v1/usage`。
    pub fn usage(&self) -> Result<Usage, TelaError> {
        self.api
            .as_ref()
            .ok_or_else(|| TelaError::NotLoggedIn {
                detail: "demo 装配没有账号".into(),
            })?
            .usage()
    }

    /// `/meta`。
    pub fn meta(&self) -> Result<Meta, TelaError> {
        match &self.api {
            Some(api) => api.meta(),
            None => ApiClient::public(tela_proto::DEFAULT_API_BASE_URL)?.meta(),
        }
    }

    // ---- GUI 登录（拆分 begin / wait，避免阻塞渲染线程） ----

    /// 当前账号快照。
    pub fn account_status(&self) -> AccountStatus {
        let auth = self.auth.as_ref();
        let store_account = self
            .lock_store()
            .ok()
            .and_then(|store| store.account().ok().flatten());
        AccountStatus {
            mode: if auth.is_some() {
                AccountMode::Real
            } else {
                AccountMode::Demo
            },
            connected: auth.is_some_and(|auth| auth.is_connected()),
            handle: auth.and_then(|auth| auth.handle()).or(store_account),
            scope: auth.and_then(|auth| auth.scope()),
            client_id_configured: self
                .client_id()
                .is_some_and(|client_id| !client_id.trim().is_empty()),
            store_error: self.credential_error(),
            db_path: self.db_path.display().to_string(),
        }
    }

    /// 绑定回环端口并返回授权 URL（很快；不等待浏览器）。
    pub fn begin_login(&self) -> Result<LoginTicket, TelaError> {
        let auth = self.auth.as_ref().ok_or_else(|| TelaError::Config {
            detail: "demo 装配不支持登录；用真实模式（去掉 --demo）或 `tela-app login`".into(),
        })?;
        let pending = auth.begin_login()?;
        let nonce = self.next_login.fetch_add(1, Ordering::SeqCst);
        let auth_url = pending.auth_url().to_string();
        *self.lock_pending_login() = Some((nonce, pending));
        Ok(LoginTicket { nonce, auth_url })
    }

    /// 等待浏览器回调并完成登录（阻塞；调用方放到后台执行器）。
    pub fn wait_login(&self, ticket: &LoginTicket) -> Result<AccountStatus, TelaError> {
        let pending = {
            let mut guard = self.lock_pending_login();
            match guard.as_ref() {
                Some((nonce, _)) if *nonce == ticket.nonce => {
                    guard.take().map(|(_, pending)| pending)
                }
                _ => None,
            }
        }
        .ok_or_else(|| TelaError::Config {
            detail: "登录票据已失效（取消、超时或已重新发起）；请再点一次登录".into(),
        })?;

        let auth = self.auth.as_ref().ok_or_else(|| TelaError::Config {
            detail: "demo 装配没有认证会话".into(),
        })?;
        let api = self.api.as_ref().ok_or_else(|| TelaError::Config {
            detail: "缺少 API 客户端".into(),
        })?;
        auth.finish_login(pending, Duration::from_secs(600))?;
        let me = api.me()?;
        self.settle_login(&me)?;
        Ok(self.account_status())
    }

    /// 放弃当前登录，关闭回环监听。
    pub fn cancel_login(&self) {
        let _ = self.lock_pending_login().take();
    }

    /// 用系统浏览器打开授权页。
    pub fn open_auth_url(&self, ticket: &LoginTicket) -> Result<(), TelaError> {
        tela_net::browser::open(&ticket.auth_url)
    }

    /// 打开文章时标记已读：先本地乐观更新并入队，再由调用方触发 `sync_now`。
    ///
    /// 尊重 `reader.mark_on_open`（缺省 true）；已经是已读时返回 `Ok(false)`。
    /// 只有 `write` 令牌才能写，否则返回 403 `insufficient_scope`。
    pub fn mark_read(&self, article_id: i64) -> Result<bool, TelaError> {
        let auth = self.auth.as_ref().ok_or_else(|| TelaError::NotLoggedIn {
            detail: "标记已读需要登录".into(),
        })?;
        if !auth.is_connected() {
            return Err(TelaError::NotLoggedIn {
                detail: "本地没有可用的 Tela 凭据".into(),
            });
        }
        if auth.scope().as_deref() != Some("write") {
            return Err(TelaError::Api {
                ctx: tela_proto::WireContext::new("local", "markRead"),
                status: 403,
                code: "insufficient_scope".into(),
                detail: "当前令牌是 read；重新登录并保留 write 权限后才能标记已读".into(),
                retry_after: None,
            });
        }

        let mut store = self.lock_store()?;
        let prefs: Vec<PrefRow> = store.rows_as("prefs")?;
        if !auto_mark_read_enabled(&prefs) {
            return Ok(false);
        }

        let key = article_id.to_string();
        if store.get_row("articles", &key)?.is_none() {
            return Err(TelaError::Content {
                article: article_id,
                content_key: key,
                detail: "本地库没有这篇文章；先同步再打开".into(),
            });
        }

        let mut state: StateRow = store.get_as("states", &key)?.unwrap_or(StateRow {
            article_id,
            read_at: None,
            marked_unread: false,
            liked_at: None,
        });
        if state.read_at.is_some() && !state.marked_unread {
            return Ok(false);
        }
        state.read_at = Some(now_ms());
        state.marked_unread = false;
        let body = serde_json::to_string(&state).map_err(|error| TelaError::Config {
            detail: format!("已读状态无法序列化：{error}"),
        })?;
        store.upsert_row("states", &key, &body)?;
        store.enqueue_mutation(&tela_proto::Mutation::mark_read(article_id))?;
        Ok(true)
    }

    /// 推完再拉一次（GUI 登录成功后用）。
    pub fn sync_now(&self) -> Result<(), TelaError> {
        self.sync_once().map(|_| ()).map_err(|error| error.source)
    }

    // ---- 同步 ----

    /// 推完再拉（阻塞）。
    #[allow(clippy::result_large_err)]
    pub fn sync_once(&self) -> Result<SyncReport, PullError> {
        self.sync_with(&PullOptions::new(&self.lang))
    }

    /// 用显式参数推完再拉。
    #[allow(clippy::result_large_err)]
    pub fn sync_with(&self, opts: &PullOptions) -> Result<SyncReport, PullError> {
        let mut store = self.lock_store().map_err(|source| PullError {
            stage: tela_sync::PullStage::Apply {
                page: 0,
                cursor: None,
                protocol: false,
            },
            applied_pages: 0,
            resume_from: None,
            source,
        })?;
        let mut engine = SyncEngine::new(&mut store, self.transport.as_ref());
        engine.sync(opts)
    }

    /// 丢掉本地游标（下一次必定从快照开始；行数据由快照覆盖）。
    pub fn reset_cursor(&self) -> Result<(), TelaError> {
        self.lock_store()?.reset_sync_state()
    }

    /// 本地同步状态。
    pub fn sync_status(&self) -> Result<SyncStatus, TelaError> {
        let store = self.lock_store()?;
        SyncStatus::snapshot(&store, None)
    }

    /// 待发变更数。
    pub fn pending_count(&self) -> Result<usize, TelaError> {
        self.lock_store()?.pending_count()
    }

    /// 把一条变更放进离线队列（下一次 `sync` 会按顺序推送）。
    pub fn enqueue_mutation(&self, mutation: &tela_proto::Mutation) -> Result<bool, TelaError> {
        self.lock_store()?.enqueue_mutation(mutation)
    }

    // ---- 本地列表 ----

    /// 订阅列表（只读本地库）。
    pub fn list_feeds(&self) -> Result<Vec<FeedItem>, TelaError> {
        let store = self.lock_store()?;
        let feeds: Vec<FeedRow> = store.rows_as("feeds")?;
        let sites: Vec<SiteRow> = store.rows_as("sites")?;
        let articles: Vec<ArticleRow> = store.rows_as("articles")?;
        let states: Vec<StateRow> = store.rows_as("states")?;
        let mut subscriptions: Vec<SubscriptionRow> = store.rows_as("subscriptions")?;

        subscriptions.retain(|subscription| subscription.deleted_at.is_none());
        subscriptions.sort_by_key(|subscription| subscription.feed_id);
        subscriptions.dedup_by_key(|subscription| subscription.feed_id);

        let feed_by_id: HashMap<i64, FeedRow> =
            feeds.into_iter().map(|feed| (feed.id, feed)).collect();
        let site_by_id: HashMap<i64, SiteRow> =
            sites.into_iter().map(|site| (site.id, site)).collect();
        let state_by_article: HashMap<i64, StateRow> = states
            .into_iter()
            .map(|state| (state.article_id, state))
            .collect();
        let now = now_ms();

        let mut result = Vec::with_capacity(subscriptions.len());
        for subscription in subscriptions {
            let feed = feed_by_id.get(&subscription.feed_id);
            let site = feed.and_then(|feed| site_by_id.get(&feed.site_id));
            let feed_articles: Vec<&ArticleRow> = articles
                .iter()
                .filter(|article| article.feed_id == subscription.feed_id)
                .collect();
            let unread = feed_articles
                .iter()
                .filter(|article| {
                    !is_read(ReadRuleInput {
                        article,
                        state: state_by_article.get(&article.id),
                        subscription: Some(&subscription),
                        horizon_days: self.horizon_days,
                        now_ms: now,
                    })
                })
                .count();
            let feed_url = feed.map(|feed| feed.feed_url.clone()).unwrap_or_default();
            let title = feed
                .and_then(|feed| feed.title.clone())
                .or_else(|| site.and_then(|site| site.title.clone()))
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| {
                    if feed_url.is_empty() {
                        format!("feed {}", subscription.feed_id)
                    } else {
                        feed_url.clone()
                    }
                });
            result.push(FeedItem {
                id: subscription.feed_id,
                title,
                feed_url,
                unread,
                article_count: feed_articles.len(),
                status: feed
                    .map(|feed| feed.status.clone())
                    .unwrap_or_else(|| "missing".to_string()),
            });
        }
        Ok(result)
    }

    /// 某个订阅下的文章（按 `sortAt` 倒序，标题取当前语言的译文）。
    pub fn list_articles(&self, feed_id: i64) -> Result<Vec<ArticleItem>, TelaError> {
        let store = self.lock_store()?;
        let mut articles: Vec<ArticleRow> = store.rows_as("articles")?;
        let states: Vec<StateRow> = store.rows_as("states")?;
        let titles: Vec<TitleRow> = store.rows_as("titles")?;
        let subscriptions: Vec<SubscriptionRow> = store.rows_as("subscriptions")?;

        let subscription = subscriptions.iter().find(|subscription| {
            subscription.feed_id == feed_id && subscription.deleted_at.is_none()
        });
        let state_by_article: HashMap<i64, StateRow> = states
            .into_iter()
            .map(|state| (state.article_id, state))
            .collect();
        let title_by_article: HashMap<i64, TitleRow> = titles
            .into_iter()
            .filter(|title| title.lang == self.lang)
            .map(|title| (title.article_id, title))
            .collect();

        articles.retain(|article| article.feed_id == feed_id);
        articles.sort_by(|left, right| {
            right
                .sort_at
                .cmp(&left.sort_at)
                .then_with(|| right.id.cmp(&left.id))
        });

        let now = now_ms();
        Ok(articles
            .into_iter()
            .map(|article| {
                let state = state_by_article.get(&article.id);
                let unread = !is_read(ReadRuleInput {
                    article: &article,
                    state,
                    subscription,
                    horizon_days: self.horizon_days,
                    now_ms: now,
                });
                let translated = title_by_article.get(&article.id);
                ArticleItem {
                    id: article.id,
                    feed_id: article.feed_id,
                    title: translated
                        .and_then(|title| title.title.clone())
                        .unwrap_or(article.title),
                    unread,
                    published_at: article.published_at,
                    excerpt: translated
                        .and_then(|title| title.excerpt.clone())
                        .or(article.excerpt),
                }
            })
            .collect())
    }

    /// 某篇文章的正文请求。
    pub fn chapter_request_for(&self, article_id: i64) -> Result<ChapterRequest, TelaError> {
        let store = self.lock_store()?;
        let key = article_id.to_string();
        let article: ArticleRow =
            store
                .get_as("articles", &key)?
                .ok_or_else(|| TelaError::Content {
                    article: article_id,
                    content_key: key.clone(),
                    detail: "本地库没有这篇文章；先运行 `tela-app sync`".into(),
                })?;
        let raw_key = article.content_key.ok_or_else(|| TelaError::Content {
            article: article_id,
            content_key: String::new(),
            detail: "文章没有正文（contentKey 为 null）".into(),
        })?;
        let content_key = normalize_content_key(&raw_key);
        if content_key.is_empty() {
            return Err(TelaError::Content {
                article: article_id,
                content_key: raw_key,
                detail: "正文对象 key 为空".into(),
            });
        }
        Ok(ChapterRequest::new(content_key, self.lang.clone()))
    }

    /// 取正文（优先本地缓存；不可变对象可长期复用）。
    pub fn load_chapter(&self, article_id: i64) -> Result<Chapter, TelaError> {
        let request = self.chapter_request_for(article_id)?;
        let cached = self.lock_store()?.get_body(&request.content_key)?;
        if let Some(body) = cached
            && let Ok(content) = serde_json::from_str::<ContentObject>(&body)
        {
            // 正文不可变，缓存永远优先；离线/令牌过期时退回原文显示，
            // 不因为拿不到译本状态就让整篇读不了。
            let translation = match self.content.translation_view(&content, &self.lang) {
                Ok(view) => view,
                Err(error) if error.needs_login() || matches!(error, TelaError::Net { .. }) => {
                    tela_content::model::TranslationView::NotAvailable
                }
                Err(error) => return Err(error),
            };
            return Ok(Chapter::new(content, translation));
        }

        let chapter = self.content.load_chapter(&request)?;
        if let Ok(body) = serde_json::to_string(&chapter.content) {
            self.lock_store()?.put_body(&request.content_key, &body)?;
        }
        Ok(chapter)
    }

    /// 请求正文翻译（需要 write 令牌）。
    pub fn request_translation(
        &self,
        article_id: i64,
        lang: Option<&str>,
    ) -> Result<tela_content::model::TranslationAnswer, TelaError> {
        self.content
            .request_translation(article_id, lang.unwrap_or(&self.lang))
    }

    // ---- 演示数据 ----

    /// 空库时写入演示订阅/文章；返回是否真的写入。
    pub fn seed_demo(&self) -> Result<bool, TelaError> {
        let mut store = self.lock_store()?;
        if store.row_count("subscriptions")? > 0 || store.row_count("articles")? > 0 {
            return Ok(false);
        }
        store.apply_sync(&demo_sync(&self.lang))?;
        if store.account()?.is_none() {
            store.set_account(DEMO_ACCOUNT)?;
        }
        Ok(true)
    }

    /// 登录完成后统一落库：handle、账号切换清理、账号标记。
    fn settle_login(&self, me: &Me) -> Result<(), TelaError> {
        let auth = self.auth.as_ref().ok_or_else(|| TelaError::Config {
            detail: "demo 装配没有认证会话".into(),
        })?;
        auth.set_handle(&me.handle)?;

        // 换了账号就不能把上一个账号的行或待发变更带过去；同一账号重连则保留本地数据。
        let previous = self.lock_store()?.account()?;
        if previous.as_deref() != Some(me.handle.as_str()) {
            self.lock_store()?.clear_account()?;
        }
        self.lock_store()?.set_account(&me.handle)?;
        Ok(())
    }

    fn lock_pending_login(&self) -> MutexGuard<'_, Option<(u64, PendingLogin)>> {
        match self.pending_login.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn lock_store(&self) -> Result<MutexGuard<'_, Store>, TelaError> {
        self.store.lock().map_err(|_| TelaError::Store {
            stage: StoreStage::Query,
            path: self.db_path.display().to_string(),
        })
    }
}

impl ShellData for AppServices {
    fn feeds(&self) -> Result<Vec<FeedItem>, TelaError> {
        self.list_feeds()
    }

    fn articles(&self, feed_id: i64) -> Result<Vec<ArticleItem>, TelaError> {
        self.list_articles(feed_id)
    }

    fn chapter_request(&self, article_id: i64) -> Result<ChapterRequest, TelaError> {
        self.chapter_request_for(article_id)
    }
}

fn open_store(path: &Path) -> Result<Store, TelaError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|_| TelaError::Store {
            stage: StoreStage::Open,
            path: path.display().to_string(),
        })?;
    }
    Store::open(path)
}

fn demo_is_default() -> bool {
    !matches!(std::env::var(ENV_DEV_SEED).ok().as_deref(), Some("0"))
}

/// `TELA_DEMO` 是否要求演示装配（默认不要求：真实登录模式）。
fn demo_requested_by_env() -> bool {
    std::env::var(ENV_DEMO)
        .ok()
        .is_some_and(|value| is_demo_value(&value))
}

/// `TELA_DEMO` 的真值：只有明确的 `1/true/yes/on` 才算 demo。
///
/// 不把「未设置/空/其它值」当 demo：发布包必须默认进登录模式；
/// `TELA_DEMO=0` 仍表示真实，兼容旧脚本。
fn is_demo_value(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// 演示快照：2 个订阅 + 4 篇文章（正常 / 长章节 / 无译文 / 已读）。
fn demo_sync(lang: &str) -> SyncResponse {
    // 用「现在」而不是固定常量：真实协议按 fetchedAt 判断地平线，
    // 固定时间会让演示文章在 30 天后全部变成「已读」。
    let now = now_ms();
    SyncResponse {
        cursor: "demo-snapshot".to_string(),
        more: false,
        reset: true,
        lang: lang.to_string(),
        types: DEFAULT_SYNC_TYPES
            .iter()
            .map(|name| name.to_string())
            .collect(),
        removed: Removed::default(),
        rows: SyncRows {
            sites: vec![
                SiteRow {
                    id: 1,
                    home_url: "https://example.com".to_string(),
                    title: Some("Tela 示例博客".to_string()),
                    description: Some("演示站点".to_string()),
                    favicon_url: None,
                    primary_lang: Some("en".to_string()),
                    reader_count: 0,
                    translation_allowed: true,
                    claimed: false,
                },
                SiteRow {
                    id: 2,
                    home_url: "https://example.com/rust".to_string(),
                    title: Some("技术随笔".to_string()),
                    description: Some("演示站点".to_string()),
                    favicon_url: None,
                    primary_lang: Some("en".to_string()),
                    reader_count: 0,
                    translation_allowed: true,
                    claimed: false,
                },
            ],
            feeds: vec![
                FeedRow {
                    id: DEMO_FEED_ID,
                    site_id: 1,
                    feed_url: "https://example.com/tela.xml".to_string(),
                    title: Some("Tela 示例博客".to_string()),
                    status: "active".to_string(),
                    last_fetched_at: Some(now),
                    last_item_at: Some(now),
                },
                FeedRow {
                    id: DEMO_FEED_ID_2,
                    site_id: 2,
                    feed_url: "https://example.com/rust-weekly.xml".to_string(),
                    title: Some("技术随笔".to_string()),
                    status: "active".to_string(),
                    last_fetched_at: Some(now),
                    last_item_at: Some(now),
                },
            ],
            subscriptions: vec![
                SubscriptionRow {
                    feed_id: DEMO_FEED_ID,
                    watermark_id: 0,
                    created_at: now,
                    deleted_at: None,
                },
                SubscriptionRow {
                    feed_id: DEMO_FEED_ID_2,
                    watermark_id: 0,
                    created_at: now,
                    deleted_at: None,
                },
            ],
            articles: vec![
                demo_article(
                    DEMO_ARTICLE_NORMAL,
                    DEMO_FEED_ID,
                    "Hello Tela：一篇文章的完整对照",
                    "欢迎来到 Tela Desktop。这一段原文来自离线 fixture，右侧是已经对齐的中文译文。",
                    MOCK_CONTENT_KEY,
                    now + 300,
                ),
                demo_article(
                    DEMO_ARTICLE_LONG,
                    DEMO_FEED_ID,
                    "长章节：120 段分页演示",
                    "这一章有 120 个段落，用来看长文分页与阅读位置保持。",
                    MOCK_LONG_CONTENT_KEY,
                    now + 200,
                ),
                demo_article(
                    DEMO_ARTICLE_NO_TRANSLATION,
                    DEMO_FEED_ID,
                    "没有译文时仍然能读原文",
                    "这一篇没有译文对象，界面应显式提示并回退到原文。",
                    MOCK_NO_TRANSLATION_CONTENT_KEY,
                    now + 100,
                ),
                demo_article(
                    DEMO_ARTICLE_READ,
                    DEMO_FEED_ID_2,
                    "Rust 周报 #1：已读文章",
                    "这是一篇已读文章，用来验证列表里的已读/未读区分。",
                    MOCK_CONTENT_KEY,
                    now + 50,
                ),
            ],
            titles: vec![
                TitleRow {
                    article_id: DEMO_ARTICLE_NORMAL,
                    lang: lang.to_string(),
                    title: Some("你好，Tela：完整对照".to_string()),
                    excerpt: Some("一段离线 fixture 的中文标题。".to_string()),
                },
                TitleRow {
                    article_id: DEMO_ARTICLE_LONG,
                    lang: lang.to_string(),
                    title: Some("长章节：120 段".to_string()),
                    excerpt: None,
                },
            ],
            states: vec![StateRow {
                article_id: DEMO_ARTICLE_READ,
                read_at: Some(now),
                marked_unread: false,
                liked_at: None,
            }],
            ..SyncRows::default()
        },
    }
}

fn demo_article(
    id: i64,
    feed_id: i64,
    title: &str,
    excerpt: &str,
    content_key: &str,
    sort_at: i64,
) -> ArticleRow {
    ArticleRow {
        id,
        feed_id,
        url: Some(format!("https://example.com/articles/{id}")),
        title: title.to_string(),
        author: Some("Tela".to_string()),
        published_at: Some(sort_at),
        fetched_at: sort_at,
        sort_at,
        source_lang: Some("en".to_string()),
        excerpt: Some(excerpt.to_string()),
        content_key: Some(content_key.to_string()),
        word_count: 120,
        reading_minutes: 2,
        like_count: 0,
        recommend_count: 0,
    }
}

impl AccountService for AppServices {
    fn status(&self) -> AccountStatus {
        self.account_status()
    }

    fn begin_login(&self) -> Result<LoginTicket, TelaError> {
        AppServices::begin_login(self)
    }

    fn wait_login(&self, ticket: &LoginTicket) -> Result<AccountStatus, TelaError> {
        AppServices::wait_login(self, ticket)
    }

    fn open_auth_url(&self, ticket: &LoginTicket) -> Result<(), TelaError> {
        AppServices::open_auth_url(self, ticket)
    }

    fn cancel_login(&self) {
        AppServices::cancel_login(self);
    }

    fn logout(&self) -> Result<AccountStatus, TelaError> {
        AppServices::logout(self)?;
        Ok(self.account_status())
    }

    fn sync_now(&self) -> Result<(), TelaError> {
        AppServices::sync_now(self)
    }

    fn mark_read(&self, article_id: i64) -> Result<bool, TelaError> {
        AppServices::mark_read(self, article_id)
    }
}

/// `reader.mark_on_open` 的客户端语义：缺省 true，只有显式 false 才关闭。
fn auto_mark_read_enabled(prefs: &[PrefRow]) -> bool {
    prefs
        .iter()
        .find(|pref| pref.key == "reader.mark_on_open")
        .and_then(|pref| pref.value.as_bool())
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pref(key: &str, value: serde_json::Value) -> PrefRow {
        PrefRow {
            key: key.to_string(),
            value,
            updated_at: 0,
        }
    }

    #[test]
    fn mark_on_open_defaults_to_true_and_honours_explicit_false() {
        assert!(auto_mark_read_enabled(&[]));
        assert!(auto_mark_read_enabled(&[pref(
            "reader.mark_on_open",
            serde_json::Value::Bool(true)
        )]));
        assert!(!auto_mark_read_enabled(&[pref(
            "reader.mark_on_open",
            serde_json::Value::Bool(false)
        )]));
        // 未知类型/未知 key 不影响默认值。
        assert!(auto_mark_read_enabled(&[pref(
            "reader.mark_on_open",
            serde_json::Value::String("yes".into())
        )]));
        assert!(auto_mark_read_enabled(&[pref(
            "reader.hide_read",
            serde_json::Value::Bool(false)
        )]));
    }

    #[test]
    fn demo_env_only_accepts_explicit_truthy_values() {
        for value in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(is_demo_value(value), "{value:?} 应进入 demo");
        }
        // 默认（未设置/空/0/其它）都必须是真实登录模式。
        for value in ["", "0", "false", "no", "off", "demo"] {
            assert!(!is_demo_value(value), "{value:?} 不应进入 demo");
        }
    }
}
