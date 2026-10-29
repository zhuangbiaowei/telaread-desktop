//! GUI 的账号抽象：状态、登录票据与 Login/Logout 能力。
//!
//! 这一层只定义纯数据与 trait，GPUI 与 `tela-app` 都依赖它：
//! - `tela-app::AppServices` 实现 [`AccountService`]（真正开回环端口、等回调、存令牌）；
//! - `tela-ui::gui::ShellView` 只调用 trait，便于用 fake 在无显示器环境测试。
//!
//! 登录被拆成 [`AccountService::begin_login`]（快：绑定端口、生成 PKCE/URL）与
//! [`AccountService::wait_login`]（慢：等浏览器回调、换令牌）。这样 GUI 可以先显示
//! 授权链接，再把等待放到后台，不阻塞渲染线程。

use tela_proto::TelaError;

/// 当前装配模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccountMode {
    /// 进程内 mock / 演示数据，没有真实账号。
    #[default]
    Demo,
    /// 真实 API + 本地库。
    Real,
}

/// GUI 状态栏需要的账号快照。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountStatus {
    /// demo 还是真实装配。
    pub mode: AccountMode,
    /// 是否有刷新令牌。
    pub connected: bool,
    /// 成员 handle。
    pub handle: Option<String>,
    /// 令牌当前的 scope（`read` / `write`）。
    pub scope: Option<String>,
    /// 是否配置了 `TELA_CLIENT_ID`。
    pub client_id_configured: bool,
    /// 凭据库不可用的原因。
    pub store_error: Option<String>,
    /// 当前本地库路径（诊断/展示）。
    pub db_path: String,
}

impl AccountStatus {
    /// 顶部一行文案。
    pub fn label(&self) -> String {
        match self.mode {
            AccountMode::Demo => "演示模式 · 无需登录".to_string(),
            AccountMode::Real if self.connected => {
                let handle = self.handle.as_deref().unwrap_or("已连接");
                match self.scope.as_deref() {
                    Some(scope) => format!("@{handle} · {scope}"),
                    None => format!("@{handle}"),
                }
            }
            AccountMode::Real => "未登录".to_string(),
        }
    }

    /// 是否应该显示登录按钮。
    pub fn can_login(&self) -> bool {
        self.mode == AccountMode::Real && !self.connected
    }

    /// 是否应该显示退出按钮。
    pub fn can_logout(&self) -> bool {
        self.mode == AccountMode::Real && self.connected
    }
}

/// 一次进行中的登录：nonce 用来让迟到的结果失效，`auth_url` 给 GUI 展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginTicket {
    /// 本次登录的序号。
    pub nonce: u64,
    /// 浏览器授权地址。
    pub auth_url: String,
}

/// 登录的界面状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LoginPhase {
    /// 空闲。
    #[default]
    Idle,
    /// 已打开回环端口，等待浏览器回调。
    Waiting(LoginTicket),
    /// 上一次登录失败。
    Error(String),
}

impl LoginPhase {
    /// 是否正在等待浏览器。
    pub fn is_waiting(&self) -> bool {
        matches!(self, Self::Waiting(_))
    }

    /// 等待中的授权地址。
    pub fn ticket(&self) -> Option<&LoginTicket> {
        match self {
            Self::Waiting(ticket) => Some(ticket),
            _ => None,
        }
    }

    /// 失败文案。
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Error(error) => Some(error),
            _ => None,
        }
    }
}

/// GUI 需要的账号能力。
///
/// 所有方法都可能被 GUI 的后台执行器调用；实现方必须保证 `wait_login` 是阻塞的、
/// 且同一时间只允许一个登录在飞（内部用 mutex/锁串行化）。
pub trait AccountService: Send + Sync {
    /// 当前状态快照。
    fn status(&self) -> AccountStatus;

    /// 绑定回环端口并生成授权 URL；很快返回。
    fn begin_login(&self) -> Result<LoginTicket, TelaError>;

    /// 等待浏览器回调、换令牌、`/v1/me` 并落库；会阻塞到授权完成或超时。
    ///
    /// 成功后返回新的状态；失败时登录票据作废，需要重新 `begin_login`。
    fn wait_login(&self, ticket: &LoginTicket) -> Result<AccountStatus, TelaError>;

    /// 用系统浏览器打开授权页。失败时 GUI 会展示 `/ 复制` 链接。
    fn open_auth_url(&self, ticket: &LoginTicket) -> Result<(), TelaError>;

    /// 放弃当前登录（关闭回环端口）。
    fn cancel_login(&self);

    /// 撤销令牌、清本地账号数据。
    fn logout(&self) -> Result<AccountStatus, TelaError>;

    /// 推完再拉一次；GUI 登录成功或标记已读后调用。
    fn sync_now(&self) -> Result<(), TelaError>;

    /// 打开文章时标记已读：先本地乐观更新并入队，返回是否真的写入。
    ///
    /// 返回 `Ok(false)` 表示无需写入（已是已读，或 `reader.mark_on_open` 关闭）。
    fn mark_read(&self, article_id: i64) -> Result<bool, TelaError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(mode: AccountMode, connected: bool) -> AccountStatus {
        AccountStatus {
            mode,
            connected,
            handle: connected.then(|| "ann".to_string()),
            scope: connected.then(|| "write".to_string()),
            client_id_configured: true,
            store_error: None,
            db_path: "/tmp/tela.db".into(),
        }
    }

    #[test]
    fn labels_and_actions_follow_mode_and_connection() {
        assert_eq!(
            status(AccountMode::Demo, false).label(),
            "演示模式 · 无需登录"
        );
        assert_eq!(status(AccountMode::Real, true).label(), "@ann · write");
        assert_eq!(status(AccountMode::Real, false).label(), "未登录");

        assert!(!status(AccountMode::Demo, false).can_login());
        assert!(status(AccountMode::Real, false).can_login());
        assert!(status(AccountMode::Real, true).can_logout());
        assert!(!status(AccountMode::Real, true).can_login());
    }

    #[test]
    fn login_phase_exposes_ticket_and_error() {
        let ticket = LoginTicket {
            nonce: 1,
            auth_url: "https://telaread.com/connect".into(),
        };
        let waiting = LoginPhase::Waiting(ticket.clone());
        assert!(waiting.is_waiting());
        assert_eq!(waiting.ticket(), Some(&ticket));
        assert!(waiting.error().is_none());

        let failed = LoginPhase::Error("denied".into());
        assert!(!failed.is_waiting());
        assert_eq!(failed.error(), Some("denied"));
        assert!(LoginPhase::default().ticket().is_none());
    }
}
