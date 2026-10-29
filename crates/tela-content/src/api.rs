//! 统一内容 API：一个 trait，两种装配（进程内 mock / 真实 HTTP）。
//!
//! 真实后端需要登录令牌，所以不再从环境变量「凭空」装配 HTTP 客户端：
//! GUI/CLI 由 `AppServices` 把已认证的 [`tela_net::ApiClient`] 传进
//! [`crate::http::HttpContentApi`]。默认（不设置 `TELA_CONTENT_API`）仍是 mock，
//! 保证离线开发、UI 联调与 CI 走同一调用路径。

use std::sync::Arc;

use tela_net::ApiClient;
use tela_proto::TelaError;

use crate::http::HttpContentApi;
use crate::mock::MockContentApi;
use crate::model::{
    Chapter, ContentObject, TranslationAnswer, TranslationObject, TranslationRow, TranslationView,
};

/// 一次章节请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterRequest {
    /// 正文对象 key（32 位十六进制）。
    pub content_key: String,
    /// 目标译文语言（`zh-Hans` / `zh-Hant` / `en` / `fr`）。
    pub lang: String,
}

impl ChapterRequest {
    /// 构造。
    pub fn new(content_key: impl Into<String>, lang: impl Into<String>) -> Self {
        Self {
            content_key: content_key.into(),
            lang: lang.into(),
        }
    }
}

/// 正文/译文取数抽象。
pub trait ContentApi: Send + Sync {
    /// 取回一章：正文对象 + 译文视图。
    ///
    /// - 空章节 `Ok(Chapter { blocks: [] })`（不是错误）；
    /// - 没有译文 `TranslationView::NotAvailable`（不是错误）；
    /// - 每日文章额度用尽返回 `TelaError::Limit`；
    /// - 其余失败统一为 [`TelaError`]。
    fn load_chapter(&self, request: &ChapterRequest) -> Result<Chapter, TelaError>;

    /// 请求为某篇文章生成正文译文（`write` 令牌）。
    fn request_translation(
        &self,
        _article_id: i64,
        _lang: &str,
    ) -> Result<TranslationAnswer, TelaError> {
        Err(TelaError::Config {
            detail: "当前内容后端不支持请求翻译".into(),
        })
    }

    /// 查询译本状态；没有译本返回 `Ok(None)`。
    fn translation_status(
        &self,
        _content_key: &str,
        _lang: &str,
    ) -> Result<Option<TranslationRow>, TelaError> {
        Ok(None)
    }

    /// 取一个译本对象（`/o/t/…` 路径）。
    fn load_translation_object(&self, _path: &str) -> Result<TranslationObject, TelaError> {
        Err(TelaError::Config {
            detail: "当前内容后端不支持译本对象".into(),
        })
    }

    /// 由正文对象构造译文视图（默认实现：查状态 → 需要时取译本对象）。
    ///
    /// 有正文缓存的后端可以只走这一条，避免重复下载不可变的正文。
    fn translation_view(
        &self,
        content: &ContentObject,
        lang: &str,
    ) -> Result<TranslationView, TelaError> {
        use crate::model::TranslationState;
        let Some(row) = self.translation_status(&content.key, lang)? else {
            return Ok(TranslationView::NotAvailable);
        };
        if row.state.is_running() || row.state == TranslationState::Waiting {
            return Ok(TranslationView::Pending {
                state: row.state,
                due_at: row.due_at,
            });
        }
        if !row.state.is_ready() {
            return Ok(TranslationView::NotAvailable);
        }
        let Some(path) = row.object else {
            return Ok(TranslationView::NotAvailable);
        };
        let object = self.load_translation_object(&path)?;
        Ok(TranslationView::from_object(content.total_blocks(), object))
    }
}

/// 内容后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentApiBackend {
    /// 进程内 fixture。
    Mock,
    /// 真实 API（需要 `ApiClient`）。
    Http,
}

/// 后端开关名。
pub const ENV_CONTENT_API: &str = "TELA_CONTENT_API";
/// 真实 API 基址开关。
pub const ENV_API_BASE_URL: &str = "TELA_API_BASE_URL";
/// 生产 API 基址。
pub const DEFAULT_API_BASE_URL: &str = tela_proto::DEFAULT_API_BASE_URL;

/// 解析后端开关；未设置/空串默认 mock，未知取值报错而不是静默降级。
pub fn resolve_backend(raw: Option<&str>) -> Result<ContentApiBackend, TelaError> {
    let value = raw.unwrap_or("").trim();
    if value.is_empty() || value.eq_ignore_ascii_case("mock") {
        Ok(ContentApiBackend::Mock)
    } else if value.eq_ignore_ascii_case("http") {
        Ok(ContentApiBackend::Http)
    } else {
        Err(config_error(format!(
            "非法取值 `{value}`：{ENV_CONTENT_API} 只接受 `mock` 或 `http`"
        )))
    }
}

/// 无认证的进程内 mock 后端。
pub fn mock_content_api() -> Box<dyn ContentApi> {
    Box::new(MockContentApi::new())
}

/// 用已认证的 `ApiClient` 装配真实后端。
pub fn http_content_api(api: Arc<ApiClient>) -> Arc<dyn ContentApi> {
    Arc::new(HttpContentApi::new(api))
}

/// 按环境变量装配（只支持 mock；`http` 需要登录，由 `AppServices` 负责）。
pub fn content_api_from_env() -> Result<Box<dyn ContentApi>, TelaError> {
    match resolve_backend(std::env::var(ENV_CONTENT_API).ok().as_deref())? {
        ContentApiBackend::Mock => Ok(mock_content_api()),
        ContentApiBackend::Http => Err(config_error(format!(
            "{ENV_CONTENT_API}=http 需要登录令牌：请用 `tela-app login` 后运行 \
             `tela-app read <article-id>`，GUI 由 AppServices 自动装配"
        ))),
    }
}

/// 内部错误：非法配置。
pub(crate) fn config_error(detail: impl Into<String>) -> TelaError {
    TelaError::Config {
        detail: detail.into(),
    }
}
