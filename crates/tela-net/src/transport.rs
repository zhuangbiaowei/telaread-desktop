//! 同步传输抽象：`tela-sync` 只认识 `pull` / `push` 两个动作。
//!
//! 真实实现是 [`crate::client::ApiClient`]；测试用 [`MockTransport`] 按脚本返回响应，
//! 两者共用同一签名，编排层不需要知道数据来自网络还是内存。

use std::collections::VecDeque;
use std::sync::Mutex;

use tela_proto::{Mutation, PushResponse, SyncResponse, TelaError, WireContext};

/// 一次 pull 的参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncQuery {
    /// 必填：译文语言。
    pub lang: String,
    /// 快照时为 `None`；否则是服务端上次给的不透明游标。
    pub cursor: Option<String>,
    /// 限定的行类型；空表示用 v1 默认 14 张表。
    pub types: Vec<String>,
}

impl SyncQuery {
    /// 快照拉取（不带 cursor）。
    pub fn snapshot(lang: impl Into<String>) -> Self {
        Self {
            lang: lang.into(),
            cursor: None,
            types: Vec::new(),
        }
    }

    /// 从某个游标续拉。
    pub fn resume(lang: impl Into<String>, cursor: impl Into<String>) -> Self {
        Self {
            lang: lang.into(),
            cursor: Some(cursor.into()),
            types: Vec::new(),
        }
    }

    /// 限定行类型。
    pub fn with_types(mut self, types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.types = types.into_iter().map(Into::into).collect();
        self
    }
}

/// 同步传输层。
pub trait SyncTransport: Send + Sync {
    /// 拉一页。
    fn pull(&self, query: &SyncQuery) -> Result<SyncResponse, TelaError>;
    /// 推一批（调用方保证 ≤ 50 条）。
    fn push(&self, mutations: &[Mutation]) -> Result<PushResponse, TelaError>;
}

/// 一次被记录下来的调用（断言脚本顺序用）。
#[derive(Debug, Clone, PartialEq)]
pub enum TransportCall {
    /// 一次 pull。
    Pull(SyncQuery),
    /// 一次 push。
    Push(Vec<Mutation>),
}

/// 内存传输层：按注入顺序返回成功或失败。
#[derive(Debug, Default)]
pub struct MockTransport {
    state: Mutex<MockState>,
}

#[derive(Debug, Default)]
struct MockState {
    pulls: VecDeque<Result<SyncResponse, TelaError>>,
    pushes: VecDeque<Result<PushResponse, TelaError>>,
    calls: Vec<TransportCall>,
}

impl MockTransport {
    /// 空脚本。
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一页成功响应。
    pub fn script_pull(&self, page: SyncResponse) {
        self.lock().pulls.push_back(Ok(page));
    }

    /// 追加一次 pull 失败。
    pub fn script_pull_error(&self, error: TelaError) {
        self.lock().pulls.push_back(Err(error));
    }

    /// 追加一次 push 成功响应。
    pub fn script_push(&self, response: PushResponse) {
        self.lock().pushes.push_back(Ok(response));
    }

    /// 追加一次 push 失败。
    pub fn script_push_error(&self, error: TelaError) {
        self.lock().pushes.push_back(Err(error));
    }

    /// 已发生的调用序列。
    pub fn calls(&self) -> Vec<TransportCall> {
        self.lock().calls.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MockState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl SyncTransport for MockTransport {
    fn pull(&self, query: &SyncQuery) -> Result<SyncResponse, TelaError> {
        let mut state = self.lock();
        state.calls.push(TransportCall::Pull(query.clone()));
        state.pulls.pop_front().unwrap_or_else(|| {
            Err(TelaError::Net {
                ctx: WireContext::new("mock://v1/sync", "sync"),
                detail: "mock pull 脚本已耗尽".into(),
            })
        })
    }

    fn push(&self, mutations: &[Mutation]) -> Result<PushResponse, TelaError> {
        let mut state = self.lock();
        state.calls.push(TransportCall::Push(mutations.to_vec()));
        state.pushes.pop_front().unwrap_or_else(|| {
            Err(TelaError::Net {
                ctx: WireContext::new("mock://v1/mutations", "mutations"),
                detail: "mock push 脚本已耗尽".into(),
            })
        })
    }
}
