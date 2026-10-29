//! 推送编排：按顺序发离线队列，按位置匹配结果，只丢弃「已被答复」的条目。
//!
//! 文档（`docs/api/v1.md` § Mutations / Offline obligations）：
//!
//! 1. 未发送的变更留在设备上，按顺序带 `mid` 与 `at`；
//! 2. 一次最多 50 条；网络失败或 5xx 时原样重发，**不改写**；
//! 3. 有答复后丢弃**所有**被答复的条目（applied 与 rejected 都算），然后 pull；
//! 4. 已发送的条目永不再编辑；后续改动是新条目、更晚的 `at`。
//!
//! 结果按位置匹配；`mid` 为 null 或与请求一致都接受，不一致视为协议违约。

use std::fmt;

use tela_proto::{MAX_MUTATIONS_PER_PUSH, MutationStatus, TelaError};

use crate::pull::SyncEngine;

/// 一次 push 的参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushOptions {
    /// 最多几批（防止一次调用无限循环）。
    pub max_batches: usize,
}

impl Default for PushOptions {
    fn default() -> Self {
        Self { max_batches: 100 }
    }
}

impl PushOptions {
    /// 最多 `batches` 批。
    pub fn with_max_batches(mut self, batches: usize) -> Self {
        self.max_batches = batches.max(1);
        self
    }
}

/// 被服务端拒绝的一条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedMutation {
    /// 请求里的 mid。
    pub mid: String,
    /// 拒绝原因（open enum，未知按 `invalid` 看待）。
    pub error: String,
}

/// 一次 push 的累计结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushReport {
    /// 成功发送的批数。
    pub batches: usize,
    /// 发送的条数。
    pub sent: usize,
    /// 服务端已应用（含幂等重放）。
    pub applied: usize,
    /// 被拒绝的条目。
    pub rejected: Vec<RejectedMutation>,
    /// 调用结束时队列里还剩多少。
    pub remaining: usize,
}

/// push 失败阶段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushStage {
    /// 传输失败（队列原样保留）。
    Transport {
        /// 第几批。
        batch: usize,
    },
    /// 响应形状与请求对不上（队列原样保留）。
    Protocol {
        /// 第几批。
        batch: usize,
        /// 说明。
        object: String,
    },
    /// 本地队列本身不合法（客户端 bug）。
    Invalid {
        /// 出问题的 mid。
        mid: String,
    },
}

impl PushStage {
    /// 三段式第 2 段。
    pub fn label(&self) -> String {
        match self {
            Self::Transport { batch } => format!("push 第 {batch} 批"),
            Self::Protocol { batch, object } => format!("push 第 {batch} 批（{object}）"),
            Self::Invalid { mid } => format!("push（{mid}）"),
        }
    }
}

/// push 失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushError {
    /// 阶段。
    pub stage: PushStage,
    /// 失败前已完成的部分。
    pub report: PushReport,
    /// 底层原因。
    pub source: TelaError,
}

impl PushError {
    /// CLI 退出码。
    pub fn exit_code(&self) -> i32 {
        self.source.exit_code()
    }

    /// 是否值得稍后原样重试。
    pub fn is_retryable(&self) -> bool {
        self.source.is_retryable()
    }
}

impl fmt::Display for PushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sync → {} → {}", self.stage.label(), self.source)?;
        if self.report.remaining > 0 {
            write!(
                f,
                "；{} 条仍在本地队列，稍后可原样重发",
                self.report.remaining
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for PushError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl SyncEngine<'_> {
    /// 按顺序把本地队列推完（或推到失败/上限）。
    #[allow(clippy::result_large_err)]
    pub fn push(&mut self, opts: &PushOptions) -> Result<PushReport, PushError> {
        let mut report = PushReport::default();
        let mut outcome = self.push_batches(opts, &mut report);
        match &mut outcome {
            Ok(report) => self.set_last_push(report.clone()),
            Err(error) => {
                self.set_last_error(error.to_string());
                // 失败时也要告诉调用方队列还剩多少，界面/CLI 才能提示「稍后重试」。
                if let Ok(remaining) = self.store.pending_count() {
                    error.report.remaining = remaining;
                }
            }
        }
        outcome
    }

    #[allow(clippy::result_large_err)]
    fn push_batches(
        &mut self,
        opts: &PushOptions,
        report: &mut PushReport,
    ) -> Result<PushReport, PushError> {
        loop {
            if report.batches >= opts.max_batches {
                report.remaining = self.store.pending_count().unwrap_or(0);
                return Ok(report.clone());
            }

            let batch = self
                .store
                .pending_mutations(MAX_MUTATIONS_PER_PUSH)
                .map_err(|source| PushError {
                    stage: PushStage::Invalid {
                        mid: "(本地队列)".into(),
                    },
                    report: report.clone(),
                    source,
                })?;
            if batch.is_empty() {
                report.remaining = 0;
                return Ok(report.clone());
            }

            if let Err(source) = tela_proto::validate_push(&batch) {
                let mid = batch
                    .first()
                    .map(|item| item.mid().to_string())
                    .unwrap_or_default();
                return Err(PushError {
                    stage: PushStage::Invalid { mid },
                    report: report.clone(),
                    source,
                });
            }

            let batch_no = report.batches + 1;
            let response = self.transport.push(&batch).map_err(|source| PushError {
                stage: PushStage::Transport { batch: batch_no },
                report: report.clone(),
                source,
            })?;

            if response.results.len() != batch.len() {
                let source = TelaError::Protocol {
                    stage: tela_proto::ProtocolStage::ShapeMismatch,
                    object: format!(
                        "push 返回 {} 条结果，请求 {} 条",
                        response.results.len(),
                        batch.len()
                    ),
                };
                return Err(PushError {
                    stage: PushStage::Protocol {
                        batch: batch_no,
                        object: format!("results 数量 {}≠{}", response.results.len(), batch.len()),
                    },
                    report: report.clone(),
                    source,
                });
            }

            let mut mids = Vec::with_capacity(batch.len());
            for (mutation, result) in batch.iter().zip(response.results.iter()) {
                if let Some(mid) = &result.mid
                    && mid != mutation.mid()
                {
                    let source = TelaError::Protocol {
                        stage: tela_proto::ProtocolStage::ShapeMismatch,
                        object: format!("push 结果 mid={mid} 与请求 {} 不一致", mutation.mid()),
                    };
                    return Err(PushError {
                        stage: PushStage::Protocol {
                            batch: batch_no,
                            object: "results.mid".into(),
                        },
                        report: report.clone(),
                        source,
                    });
                }
                match result.status {
                    MutationStatus::Applied => report.applied += 1,
                    // 未知状态按 rejected 处理：宁可回滚界面，也不要假装成功。
                    MutationStatus::Rejected | MutationStatus::Unknown => {
                        report.rejected.push(RejectedMutation {
                            mid: mutation.mid().to_string(),
                            error: result
                                .error
                                .clone()
                                .unwrap_or_else(|| "invalid".to_string()),
                        });
                    }
                }
                mids.push(mutation.mid().to_string());
            }

            self.store
                .delete_mutations(&mids)
                .map_err(|source| PushError {
                    stage: PushStage::Invalid {
                        mid: "(本地队列)".into(),
                    },
                    report: report.clone(),
                    source,
                })?;

            report.batches += 1;
            report.sent += batch.len();
            report.remaining = self.store.pending_count().unwrap_or(0);
        }
    }
}
