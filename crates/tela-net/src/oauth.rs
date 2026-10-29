//! OAuth 2.1（RFC 8252 公共原生客户端）：授权码 + PKCE（S256）。
//!
//! 服务端只支持这一种流程：没有 client secret、没有 implicit、没有 password grant
//! （`docs/api/authorization.md`）。这里实现三件事：
//!
//! 1. 生成 verifier / challenge / state（[`Pkce`]）；
//! 2. 拼授权 URL（[`authorization_url`]）；
//! 3. 换令牌、刷新、撤销（[`exchange_code`] / [`refresh_tokens`] / [`revoke_token`]）。

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tela_proto::{DEFAULT_API_BASE_URL, DEFAULT_AUTH_BASE_URL, TelaError, WireContext};

/// OAuth 配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    /// API 基址（令牌与撤销端点）。
    pub api_base: String,
    /// 授权页所在站点（需要成员的浏览器会话）。
    pub auth_base: String,
    /// 注册时拿到的 public client id（`tela_app_…`）。
    pub client_id: String,
    /// 空格分隔的 scope：`read` / `write`，可选 `email`。
    pub scopes: String,
    /// 回环回调路径，注册时是 `http://127.0.0.1/<path>`。
    pub redirect_path: String,
    /// 单次 HTTP 超时。
    pub timeout: Duration,
}

impl AuthConfig {
    /// 用 client id 构造默认配置（生产地址、`write` scope、`/callback`）。
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            api_base: DEFAULT_API_BASE_URL.to_string(),
            auth_base: DEFAULT_AUTH_BASE_URL.to_string(),
            client_id: client_id.into(),
            scopes: "write".to_string(),
            redirect_path: "/callback".to_string(),
            timeout: Duration::from_secs(20),
        }
    }

    /// 校验 client id 与服务地址。
    pub fn validate(&self) -> Result<(), TelaError> {
        if self.client_id.trim().is_empty() {
            return Err(TelaError::Config {
                detail: "缺少 TELA_CLIENT_ID：在 Tela 注册应用后把 `tela_app_…` 配进环境变量"
                    .into(),
            });
        }
        if !self.api_base.starts_with("https://") && !self.api_base.starts_with("http://") {
            return Err(TelaError::Config {
                detail: format!(
                    "api_base 必须以 http(s):// 开头，实际是 {:?}",
                    self.api_base
                ),
            });
        }
        if !self.redirect_path.starts_with('/') || self.redirect_path.contains('?') {
            return Err(TelaError::Config {
                detail: format!(
                    "redirect_path 必须是纯路径（以 / 开头、不含 query），实际是 {:?}",
                    self.redirect_path
                ),
            });
        }
        Ok(())
    }

    /// 授权页地址。
    pub fn authorize_endpoint(&self) -> String {
        format!("{}/connect", self.auth_base.trim_end_matches('/'))
    }

    /// 令牌端点。
    pub fn token_endpoint(&self) -> String {
        format!("{}/oauth/token", self.api_base.trim_end_matches('/'))
    }

    /// 撤销端点。
    pub fn revocation_endpoint(&self) -> String {
        format!("{}/oauth/revoke", self.api_base.trim_end_matches('/'))
    }
}

/// PKCE 三元组里的两个值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pkce {
    /// 43–128 字符，`A–Z a–z 0–9 - . _ ~`。
    pub verifier: String,
    /// 未填充 base64url 的 SHA-256，恒为 43 字符。
    pub challenge: String,
}

impl Pkce {
    /// 新生成一组（每次连接都必须重来，不能复用）。
    pub fn generate() -> Self {
        let mut bytes = [0u8; 48];
        rand::rng().fill_bytes(&mut bytes);
        // 64 个 base64url 字符，落在 43–128 内。
        let verifier = URL_SAFE_NO_PAD.encode(bytes);
        let challenge = Self::challenge_for(&verifier);
        Self {
            verifier,
            challenge,
        }
    }

    /// 由 verifier 算 challenge（公开给测试与 RFC 7636 向量校验）。
    pub fn challenge_for(verifier: &str) -> String {
        let digest = Sha256::digest(verifier.as_bytes());
        URL_SAFE_NO_PAD.encode(digest)
    }

    /// 校验 server 实现的 challenge 是否等于本地 verifier 的结果。
    pub fn matches(&self, challenge: &str) -> bool {
        self.challenge == challenge
    }
}

/// `state`：16 字节随机数的十六进制（32 字符）。
pub fn new_state() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 拼授权 URL。
pub fn authorization_url(
    config: &AuthConfig,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
) -> String {
    let mut url = reqwest::Url::parse(&config.authorize_endpoint())
        .unwrap_or_else(|_| reqwest::Url::parse("https://telaread.com/connect").expect("常量 URL"));
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &config.scopes)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    url.into()
}

/// 令牌响应（成功）。
#[derive(Debug, Clone, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

/// 一次成功授权的令牌对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    /// 访问令牌（`tela_at_…`）：只放内存，放 `Authorization` 头，一小时有效。
    pub access_token: String,
    /// 刷新令牌（`tela_rt_…`）：每次使用都会换新，必须妥善保存。
    pub refresh_token: String,
    /// 成员实际授予的 scope（可能被降级）。
    pub scope: String,
    /// 访问令牌过期时间（epoch 毫秒）。
    pub expires_at_ms: i64,
}

impl Tokens {
    /// 距离过期是否不足 `margin`。
    pub fn is_expiring(&self, now_ms: i64, margin: Duration) -> bool {
        let margin_ms = i64::try_from(margin.as_millis()).unwrap_or(i64::MAX);
        self.expires_at_ms <= now_ms.saturating_add(margin_ms)
    }
}

/// 构造带默认超时的 reqwest 客户端。
pub(crate) fn http_client(timeout: Duration) -> Result<reqwest::blocking::Client, TelaError> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(10)))
        .user_agent(format!("tela-desktop/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|err| TelaError::Config {
            detail: format!("无法初始化 HTTP 客户端：{err}"),
        })
}

/// 发一次 `application/x-www-form-urlencoded` POST（OAuth 端点专用）。
fn post_form(
    config: &AuthConfig,
    endpoint: &str,
    form: &[(&str, String)],
    ctx: WireContext,
) -> Result<(u16, Option<u64>, String), TelaError> {
    let client = http_client(config.timeout)?;
    let form: Vec<(String, String)> = form
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let response = client
        .post(endpoint)
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .map_err(|err| TelaError::Net {
            ctx: ctx.clone(),
            detail: err.to_string(),
        })?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok());
    let body = response.text().unwrap_or_default();
    Ok((status, retry_after, body))
}

/// OAuth 错误响应体（RFC 6749 §5.2）。
#[derive(Debug, Deserialize)]
struct OAuthErrorBody {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

fn parse_tokens(
    status: u16,
    retry_after: Option<u64>,
    body: &str,
    ctx: WireContext,
) -> Result<Tokens, TelaError> {
    if !(200..=299).contains(&status) {
        let (code, description) = serde_json::from_str::<OAuthErrorBody>(body)
            .map(|error| (error.error, error.error_description.unwrap_or_default()))
            .unwrap_or_else(|_| ("invalid_response".to_string(), body.trim().to_string()));
        let detail = if description.is_empty() {
            format!("HTTP {status} {code}")
        } else {
            format!("HTTP {status} {code}：{description}")
        };
        // 429/503 是「等一等再来」，不是「重新登录」。
        if status == 429 {
            return Err(TelaError::Limit {
                code,
                limit: None,
                resets_at: None,
                detail: match retry_after {
                    Some(seconds) => format!("{detail}；等待 {seconds} 秒"),
                    None => detail,
                },
            });
        }
        if status == 503 {
            return Err(TelaError::Api {
                ctx,
                status,
                code,
                detail,
                retry_after,
            });
        }
        return Err(TelaError::Auth { ctx, detail });
    }

    let response: TokenResponse =
        serde_json::from_str(body).map_err(|err| TelaError::Protocol {
            stage: tela_proto::ProtocolStage::ShapeMismatch,
            object: format!("token response：{err}"),
        })?;

    let now = tela_proto::now_ms();
    let refresh_token = response.refresh_token.ok_or_else(|| TelaError::Protocol {
        stage: tela_proto::ProtocolStage::ShapeMismatch,
        object: "token response.refresh_token".into(),
    })?;
    let expires_in = response.expires_in.unwrap_or(3600).max(0);
    Ok(Tokens {
        access_token: response.access_token,
        refresh_token,
        scope: response.scope.unwrap_or_else(|| "read".to_string()),
        expires_at_ms: now.saturating_add(expires_in.saturating_mul(1000)),
    })
}

/// 用授权码换令牌（授权码只能用一次，两分钟有效）。
pub fn exchange_code(
    config: &AuthConfig,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<Tokens, TelaError> {
    config.validate()?;
    let endpoint = config.token_endpoint();
    let ctx = WireContext::new(endpoint.clone(), "oauth/token");
    let (status, retry_after, body) = post_form(
        config,
        &endpoint,
        &[
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", redirect_uri.to_string()),
            ("client_id", config.client_id.clone()),
            ("code_verifier", verifier.to_string()),
        ],
        ctx.clone(),
    )?;
    parse_tokens(status, retry_after, &body, ctx)
}

/// 用刷新令牌换新的一对（旧刷新令牌立即作废）。
pub fn refresh_tokens(config: &AuthConfig, refresh_token: &str) -> Result<Tokens, TelaError> {
    config.validate()?;
    let endpoint = config.token_endpoint();
    let ctx = WireContext::new(endpoint.clone(), "oauth/token");
    let (status, retry_after, body) = post_form(
        config,
        &endpoint,
        &[
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh_token.to_string()),
            ("client_id", config.client_id.clone()),
        ],
        ctx.clone(),
    )?;
    parse_tokens(status, retry_after, &body, ctx)
}

/// 撤销令牌（任一令牌都会结束该设备的两个令牌）。200 与否都算成功。
pub fn revoke_token(config: &AuthConfig, token: &str) -> Result<(), TelaError> {
    config.validate()?;
    let endpoint = config.revocation_endpoint();
    let ctx = WireContext::new(endpoint.clone(), "oauth/revoke");
    let (status, retry_after, body) = post_form(
        config,
        &endpoint,
        &[
            ("token", token.to_string()),
            ("client_id", config.client_id.clone()),
        ],
        ctx.clone(),
    )?;
    if (200..=299).contains(&status) {
        return Ok(());
    }
    // 撤销端点对「不存在的令牌」也答 200；非 2xx 才是真的失败。
    let detail = serde_json::from_str::<OAuthErrorBody>(&body)
        .map(|error| error.error)
        .unwrap_or_else(|_| body.trim().to_string());
    Err(TelaError::Api {
        ctx,
        status,
        code: detail,
        detail: String::new(),
        retry_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7636_test_vector() {
        // RFC 7636 附录 B。
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            Pkce::challenge_for(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_pkce_is_well_formed() {
        let pkce = Pkce::generate();
        assert!((43..=128).contains(&pkce.verifier.len()));
        assert!(
            pkce.verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c))
        );
        assert_eq!(pkce.challenge.len(), 43);
        assert!(
            pkce.challenge
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
        );
        assert!(pkce.matches(&Pkce::challenge_for(&pkce.verifier)));
    }

    #[test]
    fn token_rate_limit_waits_instead_of_asking_for_a_new_login() {
        let error = parse_tokens(
            429,
            Some(30),
            r#"{"error":"rate_limited"}"#,
            WireContext::new("https://api.telaread.com/oauth/token", "oauth/token"),
        )
        .unwrap_err();
        assert_eq!(error.code(), "rate_limited");
        assert_eq!(error.exit_code(), 22);
        assert!(error.to_string().contains("30"));
    }

    #[test]
    fn token_503_waits_for_the_operator_instead_of_hammering() {
        let error = parse_tokens(
            503,
            Some(5),
            r#"{"error":"apps_paused"}"#,
            WireContext::new("https://api.telaread.com/oauth/token", "oauth/token"),
        )
        .unwrap_err();
        assert_eq!(error.code(), "apps_paused");
        assert!(!error.is_retryable(), "应用被停用时应等待，不应立刻重试");
        assert_eq!(error.exit_code(), 22);

        let transient = parse_tokens(
            503,
            Some(2),
            r#"{"error":"unavailable"}"#,
            WireContext::new("https://api.telaread.com/oauth/token", "oauth/token"),
        )
        .unwrap_err();
        assert!(transient.is_retryable());
    }

    #[test]
    fn authorization_url_names_each_parameter_once() {
        let config = AuthConfig::new("tela_app_test");
        let url = authorization_url(
            &config,
            "http://127.0.0.1:53682/callback",
            "abc",
            "challenge",
        );
        let parsed = reqwest::Url::parse(&url).unwrap();
        let pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "response_type",
                "client_id",
                "redirect_uri",
                "scope",
                "state",
                "code_challenge",
                "code_challenge_method"
            ]
        );
        assert_eq!(pairs[0].1, "code");
        assert_eq!(pairs[5].1, "challenge");
        assert_eq!(pairs[6].1, "S256");
    }
}
