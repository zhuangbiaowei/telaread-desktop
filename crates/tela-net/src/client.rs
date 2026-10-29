//! 带认证的 HTTP 客户端：JSON 请求、RFC 6750 Bearer、401 自动刷新一次、错误映射。
//!
//! 所有请求都走同一个 `send`：令牌只放 `Authorization` 头（绝不进 query），
//! 非 2xx 统一解成 `{ "error": … }` 并映射为 [`TelaError`]。
//!
//! 真实 API 的边界行为：
//! - 401 `invalid_token`：刷新一次再重发；刷新失败就是「要重新登录」；
//! - 403 `insufficient_scope` / `app_suspended`、503 `apps_paused`：保留令牌；
//! - 429 / 5xx：带 `Retry-After` 的可重试错误。

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tela_proto::{
    API_VERSION, Me, Meta, Mutation, PushRequest, PushResponse, SyncResponse, TelaError, Usage,
    WireContext,
};

use crate::oauth::http_client;
use crate::session::TokenProvider;
use crate::transport::{SyncQuery, SyncTransport};

/// 默认请求超时。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// 一次 HTTP 响应（已读成字节）。
#[derive(Debug)]
struct Response {
    status: u16,
    retry_after: Option<u64>,
    body: Vec<u8>,
}

/// 请求体。
enum Body {
    None,
    Json(Vec<u8>),
    Form(Vec<(String, String)>),
    Bytes(Vec<u8>, &'static str),
}

/// 带认证的 API 客户端。
pub struct ApiClient {
    base_url: String,
    http: reqwest::blocking::Client,
    provider: Option<Arc<dyn TokenProvider>>,
}

impl ApiClient {
    /// 已认证客户端。
    pub fn new(
        base_url: impl Into<String>,
        provider: Arc<dyn TokenProvider>,
    ) -> Result<Self, TelaError> {
        Self::with_timeout(base_url, provider, DEFAULT_TIMEOUT)
    }

    /// 带自定义超时。
    pub fn with_timeout(
        base_url: impl Into<String>,
        provider: Arc<dyn TokenProvider>,
        timeout: Duration,
    ) -> Result<Self, TelaError> {
        Ok(Self {
            base_url: normalize_base(base_url.into()),
            http: http_client(timeout)?,
            provider: Some(provider),
        })
    }

    /// 匿名客户端：只能访问 `/meta` 这类不需要令牌的端点。
    pub fn public(base_url: impl Into<String>) -> Result<Self, TelaError> {
        Ok(Self {
            base_url: normalize_base(base_url.into()),
            http: http_client(DEFAULT_TIMEOUT)?,
            provider: None,
        })
    }

    /// 测试用：注入现成的 reqwest 客户端。
    pub fn with_http(
        base_url: impl Into<String>,
        provider: Option<Arc<dyn TokenProvider>>,
        http: reqwest::blocking::Client,
    ) -> Self {
        Self {
            base_url: normalize_base(base_url.into()),
            http,
            provider,
        }
    }

    /// 当前基址。
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 请求头里的版本号（服务端会回 `Tela-API-Version`，用于诊断）。
    pub fn api_version(&self) -> u32 {
        API_VERSION
    }

    // ---- 高层端点（返回 tela-proto 类型） ----

    /// `GET /meta`（无需令牌）。
    pub fn meta(&self) -> Result<Meta, TelaError> {
        self.get_json_unauthenticated("/meta")
    }

    /// `GET /v1/me`。
    pub fn me(&self) -> Result<Me, TelaError> {
        self.get_json("/v1/me")
    }

    /// `GET /v1/usage`。
    pub fn usage(&self) -> Result<Usage, TelaError> {
        self.get_json("/v1/usage")
    }

    // ---- 通用请求 ----

    /// GET 并解析 JSON。
    pub fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, TelaError> {
        let url = self.url(path);
        let response = self.send("GET", &url, Body::None)?;
        decode_json(&url, "response", response)
    }

    /// 匿名 GET 并解析 JSON。
    pub fn get_json_unauthenticated<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, TelaError> {
        let url = self.url(path);
        let response = ensure_success(&url, self.send_once("GET", &url, Body::None, None)?)?;
        decode_json(&url, "response", response)
    }

    /// POST JSON 并解析 JSON。
    pub fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, TelaError> {
        let url = self.url(path);
        let bytes = serde_json::to_vec(body).map_err(|err| TelaError::Config {
            detail: format!("请求体无法序列化：{err}"),
        })?;
        let response = self.send("POST", &url, Body::Json(bytes))?;
        decode_json(&url, "response", response)
    }

    /// POST 表单（只有 OAuth 端点用，这里保留给测试服务器联调）。
    pub fn post_form<T: DeserializeOwned>(
        &self,
        path: &str,
        form: &[(String, String)],
    ) -> Result<T, TelaError> {
        let url = self.url(path);
        let response = self.send("POST", &url, Body::Form(form.to_vec()))?;
        decode_json(&url, "response", response)
    }

    /// POST 原始字节（OPML 导入等）。
    pub fn post_bytes<T: DeserializeOwned>(
        &self,
        path: &str,
        bytes: Vec<u8>,
        content_type: &'static str,
    ) -> Result<T, TelaError> {
        let url = self.url(path);
        let response = self.send("POST", &url, Body::Bytes(bytes, content_type))?;
        decode_json(&url, "response", response)
    }

    /// GET 原始字节（图片代理、译本对象）。
    pub fn get_bytes(&self, path: &str) -> Result<Vec<u8>, TelaError> {
        let url = self.url(path);
        let response = self.send("GET", &url, Body::None)?;
        Ok(response.body)
    }

    /// 把相对路径拼到基址上；已经是绝对 URL 时原样使用。
    fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            return path.to_string();
        }
        if path.starts_with('/') {
            format!("{}{}", self.base_url, path)
        } else {
            format!("{}/{}", self.base_url, path)
        }
    }

    /// 带令牌发一次；401 时刷新并重发一次。
    fn send(&self, method: &str, url: &str, body: Body) -> Result<Response, TelaError> {
        let Some(provider) = &self.provider else {
            return Err(TelaError::NotLoggedIn {
                detail: format!("{method} {url} 需要登录令牌"),
            });
        };
        let token = provider.access_token()?;
        let first = self.send_once(method, url, clone_body(&body), Some(token))?;
        if first.status != 401 {
            return ensure_success(url, first);
        }

        // 401 只重试一次：刷新后还 401 就是真的没权限/令牌已结束。
        provider.refresh()?;
        let token = provider.access_token()?;
        ensure_success(url, self.send_once(method, url, body, Some(token))?)
    }

    fn send_once(
        &self,
        method: &str,
        url: &str,
        body: Body,
        token: Option<String>,
    ) -> Result<Response, TelaError> {
        let ctx = WireContext::new(url.to_string(), endpoint_name(url));
        let method =
            reqwest::Method::from_bytes(method.as_bytes()).map_err(|err| TelaError::Config {
                detail: format!("非法 HTTP 方法 {method}：{err}"),
            })?;
        let mut builder = self
            .http
            .request(method, url)
            .header("Accept", "application/json");
        if let Some(token) = token {
            builder = builder.bearer_auth(token);
        }
        builder = match body {
            Body::None => builder,
            Body::Json(bytes) => builder
                .header("Content-Type", "application/json")
                .body(bytes),
            Body::Form(form) => builder.form(&form),
            Body::Bytes(bytes, content_type) => {
                builder.header("Content-Type", content_type).body(bytes)
            }
        };

        let response = builder.send().map_err(|err| TelaError::Net {
            ctx: ctx.clone(),
            detail: err.to_string(),
        })?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok());
        let body = response.bytes().map_err(|err| TelaError::Net {
            ctx: ctx.clone(),
            detail: format!("读取响应失败：{err}"),
        })?;

        Ok(Response {
            status,
            retry_after,
            body: body.to_vec(),
        })
    }
}

/// 2xx 放行，其余映射成错误。
fn ensure_success(url: &str, response: Response) -> Result<Response, TelaError> {
    if (200..=299).contains(&response.status) {
        return Ok(response);
    }
    let ctx = WireContext::new(url.to_string(), endpoint_name(url));
    Err(map_api_error(
        &ctx,
        response.status,
        response.retry_after,
        &response.body,
    ))
}

impl SyncTransport for ApiClient {
    fn pull(&self, query: &SyncQuery) -> Result<SyncResponse, TelaError> {
        let mut url =
            reqwest::Url::parse(&format!("{}/v1/sync", self.base_url)).map_err(|err| {
                TelaError::Config {
                    detail: format!("base_url 无法解析：{err}"),
                }
            })?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("lang", &query.lang);
            if let Some(cursor) = &query.cursor {
                pairs.append_pair("cursor", cursor);
            }
            if !query.types.is_empty() {
                pairs.append_pair("types", &query.types.join(","));
            }
        }
        let response = self.send("GET", url.as_str(), Body::None)?;
        decode_json(url.as_str(), "sync", response)
    }

    fn push(&self, mutations: &[Mutation]) -> Result<PushResponse, TelaError> {
        tela_proto::validate_push(mutations)?;
        let url = self.url("/v1/mutations");
        let bytes = serde_json::to_vec(&PushRequest {
            mutations: mutations.to_vec(),
        })
        .map_err(|err| TelaError::Config {
            detail: format!("push 请求体无法序列化：{err}"),
        })?;
        let response = self.send("POST", &url, Body::Json(bytes))?;
        decode_json(&url, "mutations", response)
    }
}

fn clone_body(body: &Body) -> Body {
    match body {
        Body::None => Body::None,
        Body::Json(bytes) => Body::Json(bytes.clone()),
        Body::Form(form) => Body::Form(form.clone()),
        Body::Bytes(bytes, content_type) => Body::Bytes(bytes.clone(), content_type),
    }
}

fn decode_json<T: DeserializeOwned>(
    url: &str,
    object: &str,
    response: Response,
) -> Result<T, TelaError> {
    serde_json::from_slice(&response.body).map_err(|err| TelaError::Protocol {
        stage: tela_proto::ProtocolStage::ShapeMismatch,
        object: format!("{}（{url}；HTTP {}）：{err}", object, response.status),
    })
}

/// API 错误体。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiErrorBody {
    error: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    resets_at: Option<i64>,
}

/// 把非 2xx 响应映射成 [`TelaError`]。
fn map_api_error(
    ctx: &WireContext,
    status: u16,
    retry_after: Option<u64>,
    body: &[u8],
) -> TelaError {
    let parsed = serde_json::from_slice::<ApiErrorBody>(body).ok();
    let code = parsed
        .as_ref()
        .map(|body| body.error.clone())
        .unwrap_or_else(|| format!("http_{status}"));
    let detail = parsed
        .as_ref()
        .and_then(|body| {
            body.message
                .clone()
                .or_else(|| body.error_description.clone())
        })
        .unwrap_or_default();

    if matches!(code.as_str(), "rate_limited" | "feed_limit" | "post_limit") {
        return TelaError::Limit {
            code,
            limit: parsed.as_ref().and_then(|body| body.limit),
            resets_at: parsed.as_ref().and_then(|body| body.resets_at),
            detail: if detail.is_empty() {
                format!("HTTP {status}")
            } else {
                detail
            },
        };
    }

    TelaError::Api {
        ctx: ctx.clone(),
        status,
        code,
        detail,
        retry_after,
    }
}

fn endpoint_name(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    path.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .skip(1)
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_base(base: String) -> String {
    base.trim().trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_join_without_double_slash() {
        let client = ApiClient::public("https://api.telaread.com/").unwrap();
        assert_eq!(client.url("/v1/me"), "https://api.telaread.com/v1/me");
        assert_eq!(client.url("v1/me"), "https://api.telaread.com/v1/me");
        assert_eq!(
            client.url("/o/t/abc/en/1.json"),
            "https://api.telaread.com/o/t/abc/en/1.json"
        );
    }

    #[test]
    fn api_errors_map_to_codes() {
        let ctx = WireContext::new("https://api.telaread.com/v1/sync", "sync");
        let error = map_api_error(
            &ctx,
            403,
            None,
            br#"{"error":"insufficient_scope","message":"write required"}"#,
        );
        assert_eq!(error.code(), "insufficient_scope");
        assert_eq!(error.http_status(), 403);
        assert_eq!(error.exit_code(), 21);

        let limit = map_api_error(
            &ctx,
            429,
            Some(30),
            br#"{"error":"post_limit","limit":100,"resetsAt":1761609600000}"#,
        );
        match limit {
            TelaError::Limit {
                limit: Some(100),
                resets_at: Some(1_761_609_600_000),
                ..
            } => {}
            other => panic!("应映射为 Limit，实际 {other:?}"),
        }

        let paused = map_api_error(&ctx, 503, Some(5), br#"{"error":"apps_paused"}"#);
        assert!(!paused.is_retryable(), "应用被停用时应等待而不是立刻重试");
        assert_eq!(paused.exit_code(), 22);
    }
}
