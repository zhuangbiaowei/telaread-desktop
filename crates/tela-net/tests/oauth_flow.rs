//! OAuth 2.1 端到端：回环回调、state/iss 校验、换令牌、刷新令牌轮换。

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use common::MockServer;
use tela_net::callback::PendingLogin;
use tela_net::oauth::AuthConfig;
use tela_net::{AuthSession, MemoryTokenStore, TokenStore};

const TOKENS: &str = r#"{"access_token":"tela_at_first","token_type":"Bearer",
    "expires_in":3600,"refresh_token":"tela_rt_first","scope":"write email"}"#;

fn config(base: &str) -> AuthConfig {
    let mut config = AuthConfig::new("tela_app_test");
    config.api_base = base.to_string();
    config.timeout = Duration::from_secs(5);
    config
}

fn redirect_parts(redirect: &str) -> (String, u16, String) {
    let url = reqwest::Url::parse(redirect).unwrap();
    (
        url.host_str().unwrap().to_string(),
        url.port().unwrap(),
        url.path().to_string(),
    )
}

/// 模拟浏览器访问回环地址。
fn deliver_callback(redirect: &str, state: &str, code: &str) {
    let (host, port, path) = redirect_parts(redirect);
    let mut stream = TcpStream::connect((host.as_str(), port)).unwrap();
    let request = format!(
        "GET {path}?code={code}&state={state}&iss=https%3A%2F%2Fapi.telaread.com HTTP/1.1\r\n\
         Host: {host}:{port}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("Tela Desktop"), "{response}");
}

#[test]
fn loopback_login_exchanges_the_code_and_keeps_the_refresh_token() {
    let server = MockServer::start(1, |request| {
        assert_eq!(request.path, "/oauth/token");
        let form = request.form();
        assert_eq!(
            form.get("grant_type").map(String::as_str),
            Some("authorization_code")
        );
        assert_eq!(form.get("code").map(String::as_str), Some("the-code"));
        assert_eq!(
            form.get("client_id").map(String::as_str),
            Some("tela_app_test")
        );
        assert!(
            form.get("code_verifier")
                .is_some_and(|verifier| verifier.len() >= 43),
            "必须带 PKCE verifier"
        );
        (200, TOKENS.to_string())
    });

    let pending = PendingLogin::begin(&config(&server.base_url())).unwrap();
    let auth_url = reqwest::Url::parse(pending.auth_url()).unwrap();
    let state = auth_url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .unwrap();
    assert_eq!(
        auth_url
            .query_pairs()
            .find(|(key, _)| key == "code_challenge_method")
            .map(|(_, value)| value.into_owned())
            .as_deref(),
        Some("S256")
    );

    let redirect = pending.redirect_uri().to_string();
    let callback_state = state.clone();
    let callback =
        std::thread::spawn(move || deliver_callback(&redirect, &callback_state, "the-code"));

    let answer = pending.wait(Duration::from_secs(5)).unwrap();
    assert_eq!(answer.code, "the-code");
    callback.join().unwrap();

    let store = Arc::new(MemoryTokenStore::new());
    let tokens = pending
        .complete(&answer, store.as_ref(), "default")
        .unwrap();
    assert_eq!(tokens.access_token, "tela_at_first");
    assert_eq!(tokens.refresh_token, "tela_rt_first");
    assert_eq!(tokens.scope, "write email");

    let saved = store.load("default").unwrap().expect("刷新令牌必须落存储");
    assert_eq!(saved.refresh_token, "tela_rt_first");
    assert_eq!(saved.scope, "write email");
}

#[test]
fn a_wrong_state_is_dropped_without_a_token_request() {
    let server = MockServer::start(1, |_| (200, TOKENS.to_string()));
    let pending = PendingLogin::begin(&config(&server.base_url())).unwrap();
    let redirect = pending.redirect_uri().to_string();
    let callback =
        std::thread::spawn(move || deliver_callback(&redirect, "wrong-state", "the-code"));

    let error = pending.wait(Duration::from_secs(5)).unwrap_err();
    callback.join().unwrap();
    assert!(error.to_string().contains("state"), "{error}");
    assert_eq!(server.requests().len(), 0, "state 不对就不能去换令牌");
}

#[test]
fn refresh_rotates_the_stored_token_before_use() {
    let server = MockServer::start(1, |request| {
        let form = request.form();
        assert_eq!(
            form.get("grant_type").map(String::as_str),
            Some("refresh_token")
        );
        assert_eq!(
            form.get("refresh_token").map(String::as_str),
            Some("tela_rt_old")
        );
        (
            200,
            r#"{"access_token":"tela_at_new","token_type":"Bearer","expires_in":3600,
                "refresh_token":"tela_rt_new","scope":"write"}"#
                .to_string(),
        )
    });

    let store = Arc::new(MemoryTokenStore::new());
    store
        .save(
            "default",
            &tela_net::StoredCredential {
                refresh_token: "tela_rt_old".into(),
                scope: "write".into(),
                handle: Some("ann".into()),
            },
        )
        .unwrap();
    let session = AuthSession::load(config(&server.base_url()), store.clone()).unwrap();
    session.refresh().unwrap();

    assert_eq!(
        session.current_access_token().as_deref(),
        Some("tela_at_new")
    );
    let saved = store.load("default").unwrap().unwrap();
    assert_eq!(saved.refresh_token, "tela_rt_new", "旧令牌已被消费");
    assert_eq!(
        saved.handle.as_deref(),
        Some("ann"),
        "handle 不能在刷新时丢失"
    );
}

#[derive(Debug)]
struct UnavailableStore;

impl TokenStore for UnavailableStore {
    fn load(
        &self,
        _account: &str,
    ) -> Result<Option<tela_net::StoredCredential>, tela_proto::TelaError> {
        Err(tela_proto::TelaError::Config {
            detail: "无法访问系统凭据库（No secret service）".into(),
        })
    }

    fn save(
        &self,
        _account: &str,
        _credential: &tela_net::StoredCredential,
    ) -> Result<(), tela_proto::TelaError> {
        Err(tela_proto::TelaError::Config {
            detail: "无法写入系统凭据库".into(),
        })
    }

    fn delete(&self, _account: &str) -> Result<(), tela_proto::TelaError> {
        Ok(())
    }
}

#[test]
fn a_missing_credential_store_does_not_break_offline_commands() {
    let session =
        AuthSession::load(config("http://127.0.0.1:1"), Arc::new(UnavailableStore)).unwrap();
    assert!(!session.is_connected());
    assert!(
        session
            .store_error()
            .is_some_and(|error| error.contains("secret service"))
    );

    // 真正登录时才报错，并提示替代路径。
    let error = session
        .login(false, Duration::from_millis(10), |_| {})
        .unwrap_err();
    assert!(error.to_string().contains("--ephemeral"), "{error}");
}
