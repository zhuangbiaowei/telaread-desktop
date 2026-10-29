//! `tela-sync` — 同步编排：拉取、推送、离线队列与状态。
//!
//! 依赖方向：`tela-sync → {tela-store, tela-net} → tela-proto`。本层不写 HTTP 细节，
//! 也不碰 UI；`tela-app` 把 `Store` 与 `ApiClient` 装进来即可。
//!
//! ## 一次同步的顺序
//!
//! 1. 先把本地待发队列推完（离线积压优先落地）；
//! 2. 再按当前语言/类型拉取，直到 `more=false`；
//! 3. 推送失败不清队列，拉取成功仍会更新本地行——两者互不阻塞。

mod pull;
mod push;
mod status;

pub use pull::{
    DEFAULT_MAX_ATTEMPTS, DEFAULT_MAX_PAGES, DEFAULT_RETRY_DELAY, PullError, PullOptions,
    PullReport, PullStage, SyncEngine,
};
pub use push::{PushError, PushOptions, PushReport, PushStage, RejectedMutation};
pub use status::SyncStatus;

use pull::PullOptions as _PullOptions;

/// 一次「推完再拉」的合并结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// 推送结果（推送失败时为失败前的部分结果）。
    pub push: Option<PushReport>,
    /// 推送失败文案（拉取仍会继续）。
    pub push_error: Option<String>,
    /// 拉取结果。
    pub pull: PullReport,
}

impl SyncEngine<'_> {
    /// 推完再拉。
    ///
    /// 推送失败不会阻止拉取：离线队列留在本地，下次再推；但拉取失败会向上返回，
    /// 因为它决定了本地视图是否可信。
    #[allow(clippy::result_large_err)]
    pub fn sync(&mut self, opts: &_PullOptions) -> Result<SyncReport, PullError> {
        let (push, push_error) = match self.push(&PushOptions::default()) {
            Ok(report) => (Some(report), None),
            Err(error) => (Some(error.report.clone()), Some(error.to_string())),
        };
        let pull = self.pull(opts)?;
        Ok(SyncReport {
            push,
            push_error,
            pull,
        })
    }
}
