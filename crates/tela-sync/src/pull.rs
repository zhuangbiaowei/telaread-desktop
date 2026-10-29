//! 拉取编排：快照/增量、`more` 续拉、语言与类型变化时自动重新快照。
//!
//! 文档（`docs/api/v1.md` § Sync）对客户端的硬要求：
//!
//! - `reset: true` 是快照：先丢该账号全部行，再接收本页；
//! - `more: true` 立即用新 cursor 续拉；
//! - cursor 与行**同一次写**，且原样送回；
//! - 换语言或换行类型后旧 cursor 不再适用，必须从快照重来——服务端也会以
//!   `reset` 回应，但客户端先自己判断能少一圈无效请求；
//! - 只有网络/5xx 值得在同一页重试；401/403/429/协议错误立即终止。

use std::fmt;
use std::time::Duration;

use tela_net::{SyncQuery, SyncTransport};
use tela_proto::{DEFAULT_SYNC_TYPES, ProtocolStage, TelaError};
use tela_store::{ApplyOutcome, Store};

use crate::status::SyncStatus;

/// 每页最多尝试次数（含首次）。
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;
/// 单次拉取最多页数（`more` 恒真时的死循环保护）。
pub const DEFAULT_MAX_PAGES: usize = 1000;
/// 重试前的等待。
pub const DEFAULT_RETRY_DELAY: Duration = Duration::from_millis(300);

/// 拉取参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullOptions {
    /// 必填：译文语言（`zh-Hans` / `zh-Hant` / `en` / `fr`）。
    pub lang: String,
    /// 限定的行类型；空表示默认 14 张。
    pub types: Vec<String>,
    /// 每页最多尝试次数。
    pub max_attempts: u32,
    /// 单次最多页数。
    pub max_pages: usize,
    /// 重试间隔。
    pub retry_delay: Duration,
}

impl PullOptions {
    /// 以语言构造默认参数。
    pub fn new(lang: impl Into<String>) -> Self {
        Self {
            lang: lang.into(),
            types: Vec::new(),
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            max_pages: DEFAULT_MAX_PAGES,
            retry_delay: DEFAULT_RETRY_DELAY,
        }
    }

    /// 限定行类型。
    pub fn with_types(mut self, types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.types = types.into_iter().map(Into::into).collect();
        self
    }

    /// 每页尝试次数（至少 1）。
    pub fn with_max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts.max(1);
        self
    }

    /// 页数上限（至少 1）。
    pub fn with_max_pages(mut self, pages: usize) -> Self {
        self.max_pages = pages.max(1);
        self
    }

    /// 重试间隔（测试里设 0）。
    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    /// 实际使用的行类型。
    pub fn effective_types(&self) -> Vec<String> {
        if self.types.is_empty() {
            DEFAULT_SYNC_TYPES
                .iter()
                .map(|name| name.to_string())
                .collect()
        } else {
            self.types.clone()
        }
    }
}

/// 一次拉取的累计结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullReport {
    /// 成功落库的页数。
    pub pages: usize,
    /// 写入/覆盖的行数。
    pub rows: usize,
    /// `removed.articles` 删除的文章数。
    pub removed: usize,
    /// 级联删除的行数（退订、关闭翻译）。
    pub cascaded: usize,
    /// 本次是否从快照开始。
    pub reset: bool,
    /// 结束时的游标；无页成功时为起始游标。
    pub cursor: Option<String>,
    /// 含重试的尝试次数。
    pub attempts: u32,
    /// 本次拉取的语言。
    pub lang: String,
}

/// 失败阶段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullStage {
    /// 传输失败（重试用尽）。
    Transport {
        /// 第几页。
        page: usize,
        /// 使用的游标。
        cursor: Option<String>,
        /// 尝试次数。
        attempts: u32,
    },
    /// 落库/协议失败。
    Apply {
        /// 第几页。
        page: usize,
        /// 使用的游标。
        cursor: Option<String>,
        /// 是否协议违约。
        protocol: bool,
    },
    /// `more` 长期不收敛。
    PageBudget {
        /// 已达游标。
        cursor: Option<String>,
        /// 上限。
        max_pages: usize,
    },
}

impl PullStage {
    /// 三段式第 2 段。
    pub fn label(&self) -> String {
        match self {
            Self::Transport {
                page,
                cursor,
                attempts,
            } => format!(
                "pull 第 {page} 页（cursor={}，尝试 {attempts} 次）",
                cursor.as_deref().unwrap_or("(snapshot)")
            ),
            Self::Apply {
                page,
                cursor,
                protocol,
            } => format!(
                "pull 第 {page} 页{}（cursor={}）",
                if *protocol { "协议校验" } else { "落库" },
                cursor.as_deref().unwrap_or("(snapshot)")
            ),
            Self::PageBudget { cursor, max_pages } => format!(
                "pull 分页（cursor={}，上限 {max_pages} 页）",
                cursor.as_deref().unwrap_or("(snapshot)")
            ),
        }
    }
}

/// 拉取失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullError {
    /// 阶段。
    pub stage: PullStage,
    /// 失败前已落库页数。
    pub applied_pages: usize,
    /// 可续跑的游标。
    pub resume_from: Option<String>,
    /// 底层原因。
    pub source: TelaError,
}

impl PullError {
    /// CLI 退出码。
    pub fn exit_code(&self) -> i32 {
        match self.stage {
            PullStage::PageBudget { .. } => 12,
            _ => self.source.exit_code(),
        }
    }

    /// 是否值得稍后重试。
    pub fn is_retryable(&self) -> bool {
        self.source.is_retryable()
    }
}

impl fmt::Display for PullError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sync → {} → ", self.stage.label())?;
        match &self.stage {
            PullStage::PageBudget { max_pages, .. } => {
                write!(f, "连续 {max_pages} 页 more 仍为 true；核对服务端游标推进")
            }
            _ => write!(
                f,
                "{}；已落库 {} 页，可续跑游标 {}",
                self.source,
                self.applied_pages,
                self.resume_from.as_deref().unwrap_or("(无)")
            ),
        }
    }
}

impl std::error::Error for PullError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// 同步引擎：唯一同时持有 `Store` 与 `SyncTransport` 的地方。
pub struct SyncEngine<'a> {
    pub(crate) store: &'a mut Store,
    pub(crate) transport: &'a dyn SyncTransport,
    last_pull: Option<PullReport>,
    last_push: Option<crate::push::PushReport>,
    last_error: Option<String>,
}

impl<'a> SyncEngine<'a> {
    /// 装配。
    pub fn new(store: &'a mut Store, transport: &'a dyn SyncTransport) -> Self {
        Self {
            store,
            transport,
            last_pull: None,
            last_push: None,
            last_error: None,
        }
    }

    /// 只读访问本地库。
    pub fn store(&self) -> &Store {
        self.store
    }

    /// 记录最近一次 push 结果（`push` 模块调用）。
    pub(crate) fn set_last_push(&mut self, report: crate::push::PushReport) {
        self.last_push = Some(report);
    }

    /// 记录最近一次失败文案。
    pub(crate) fn set_last_error(&mut self, message: String) {
        self.last_error = Some(message);
    }

    /// 当前状态快照（游标、行数、待发队列、最近结果）。
    pub fn status(&self) -> Result<SyncStatus, TelaError> {
        SyncStatus::snapshot(self.store, self.last_error.clone())
    }

    /// 从本地游标起连续拉取，直到 `more=false`。
    #[allow(clippy::result_large_err)]
    pub fn pull(&mut self, opts: &PullOptions) -> Result<PullReport, PullError> {
        let outcome = self.pull_pages(opts);
        match &outcome {
            Ok(report) => {
                self.last_pull = Some(report.clone());
                self.last_error = None;
            }
            Err(error) => self.last_error = Some(error.to_string()),
        }
        outcome
    }

    #[allow(clippy::result_large_err)]
    fn pull_pages(&mut self, opts: &PullOptions) -> Result<PullReport, PullError> {
        let types = opts.effective_types();
        let mut cursor = self
            .initial_cursor(&opts.lang, &types)
            .map_err(|source| PullError {
                stage: PullStage::Apply {
                    page: 0,
                    cursor: None,
                    protocol: false,
                },
                applied_pages: 0,
                resume_from: None,
                source,
            })?;

        let mut report = PullReport {
            cursor: cursor.clone(),
            lang: opts.lang.clone(),
            ..PullReport::default()
        };

        loop {
            let page_no = report.pages + 1;
            let query = SyncQuery {
                lang: opts.lang.clone(),
                cursor: cursor.clone(),
                types: types.clone(),
            };

            let mut attempt = 0u32;
            let page = loop {
                attempt += 1;
                report.attempts += 1;
                match self.transport.pull(&query) {
                    Ok(page) => break page,
                    Err(source) => {
                        if source.is_retryable() && attempt < opts.max_attempts {
                            if !opts.retry_delay.is_zero() {
                                std::thread::sleep(opts.retry_delay);
                            }
                            continue;
                        }
                        return Err(PullError {
                            stage: PullStage::Transport {
                                page: page_no,
                                cursor: cursor.clone(),
                                attempts: attempt,
                            },
                            applied_pages: report.pages,
                            resume_from: cursor,
                            source,
                        });
                    }
                }
            };

            let reset = page.reset;
            let more = page.more;
            let outcome: ApplyOutcome =
                self.store.apply_sync(&page).map_err(|source| PullError {
                    stage: PullStage::Apply {
                        page: page_no,
                        cursor: cursor.clone(),
                        protocol: matches!(source, TelaError::Protocol { .. }),
                    },
                    applied_pages: report.pages,
                    resume_from: cursor.clone(),
                    source,
                })?;

            report.pages += 1;
            report.rows += outcome.rows;
            report.removed += outcome.removed;
            report.cascaded += outcome.cascaded;
            report.reset |= reset;
            report.cursor = Some(outcome.cursor.clone());
            cursor = report.cursor.clone();

            if !more {
                return Ok(report);
            }
            if report.pages >= opts.max_pages {
                return Err(PullError {
                    stage: PullStage::PageBudget {
                        cursor,
                        max_pages: opts.max_pages,
                    },
                    applied_pages: report.pages,
                    resume_from: report.cursor.clone(),
                    source: TelaError::Protocol {
                        stage: ProtocolStage::Cursor,
                        object: "more 连续为 true".into(),
                    },
                });
            }
        }
    }

    /// 本地 cursor 是否还能用于这次 lang/types。
    fn initial_cursor(&self, lang: &str, types: &[String]) -> Result<Option<String>, TelaError> {
        let stored_lang = self.store.sync_lang()?;
        let stored_types = self.store.sync_types()?;
        if stored_lang.as_deref() != Some(lang) {
            return Ok(None);
        }
        if !stored_types.is_empty() && !same_set(&stored_types, types) {
            return Ok(None);
        }
        self.store.cursor()
    }
}

fn same_set(left: &[String], right: &[String]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort();
    right.sort();
    left == right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_sets_compare_order_insensitively() {
        let a = vec!["articles".to_string(), "feeds".to_string()];
        let b = vec!["feeds".to_string(), "articles".to_string()];
        assert!(same_set(&a, &b));
        assert!(!same_set(&a, &["feeds".to_string()]));
    }
}
