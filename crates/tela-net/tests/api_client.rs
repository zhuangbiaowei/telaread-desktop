//! 真实 HTTP 客户端：Bearer 头、401 刷新重试、错误映射、sync 查询编码。

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::MockServer;
use tela_net::{ApiClient, StaticToken, SyncQuery, SyncTransport, TokenProvider};
use tela_proto::{Me, SyncResponse, TelaError};

const ME: &str = include_str!("../../../tests/fixtures/api/me.body.json");
const SNAPSHOT: &str = include_str!("../../../tests/fixtures/api/sync-snapshot.body.json");

#[derive(Debug)]
struct RotatingToken {
    refreshed: AtomicUsize,
}

impl TokenProvider for RotatingToken {
    fn access_token(&self) -> Result<String, TelaError> {
        Ok(if self.refreshed.load(Ordering::SeqCst) == 0 {
            "expired".to_string()
        } else {
            "fresh".to_string()
        })
    }

    fn refresh(&self) -> Result<(), TelaError> {
        self.refreshed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn a_401_refreshes_once_and_retries_with_the_new_token() {
    let server = MockServer::start(2, |request| {
        if request.header("authorization") == Some("Bearer fresh") {
            (200, ME.to_string())
        } else {
            (401, r#"{"error":"invalid_token"}"#.to_string())
        }
    });
    let provider = Arc::new(RotatingToken {
        refreshed: AtomicUsize::new(0),
    });
    let client =
        ApiClient::with_timeout(server.base_url(), provider.clone(), Duration::from_secs(5))
            .unwrap();

    let me: Me = client.me().unwrap();
    assert_eq!(me.handle, "ann");
    assert_eq!(provider.refreshed.load(Ordering::SeqCst), 1);

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].header("authorization"), Some("Bearer expired"));
    assert_eq!(requests[1].header("authorization"), Some("Bearer fresh"));
}

#[test]
fn app_suspension_maps_to_an_api_error_with_the_documented_exit_code() {
    let server = MockServer::start(1, |_| {
        (
            403,
            r#"{"error":"app_suspended","message":"cut off"}"#.to_string(),
        )
    });
    let client = ApiClient::with_timeout(
        server.base_url(),
        Arc::new(StaticToken("t".into())),
        Duration::from_secs(5),
    )
    .unwrap();

    let error = client.me().unwrap_err();
    assert_eq!(error.code(), "app_suspended");
    assert_eq!(error.http_status(), 403);
    assert_eq!(error.exit_code(), 21);
    assert!(error.to_string().contains("cut off"));
}

#[test]
fn sync_query_percent_encodes_the_opaque_cursor_and_keeps_the_bearer_header() {
    let server = MockServer::start(1, |_| (200, SNAPSHOT.to_string()));
    let client = ApiClient::with_timeout(
        server.base_url(),
        Arc::new(StaticToken("secret-token".into())),
        Duration::from_secs(5),
    )
    .unwrap();

    let page: SyncResponse = client
        .pull(&SyncQuery {
            lang: "zh-Hans".into(),
            cursor: Some("a+b/c=".into()),
            types: vec!["articles".into(), "states".into()],
        })
        .unwrap();
    assert!(page.reset);
    assert_eq!(page.rows.articles.len(), 4);

    let request = &server.requests()[0];
    let query = request.query();
    assert_eq!(query.get("lang").map(String::as_str), Some("zh-Hans"));
    assert_eq!(query.get("cursor").map(String::as_str), Some("a+b/c="));
    assert_eq!(
        query.get("types").map(String::as_str),
        Some("articles,states")
    );
    assert_eq!(request.header("authorization"), Some("Bearer secret-token"));
    assert!(!request.path.contains("access_token"), "令牌绝不能进 query");
}

#[test]
fn public_meta_needs_no_token() {
    let meta = r#"{"api":"tela","current":1,"oldest":1,"versions":[],"features":{},
        "limits":{"pushMutations":50,"pushBytes":1048576,"pageRows":1000,
        "bodiesPerRequest":25,"horizonDays":30,"readingLanguages":["en"],
        "translationsPerDay":null,"feedsPerDay":null,"maxSubscriptions":null,
        "unsubscribedPostsPerDay":null},"oauth":{"metadata":"https://api.telaread.com/.well-known/oauth-authorization-server"}}"#;
    let server = MockServer::start(1, move |_| (200, meta.to_string()));
    let client = ApiClient::public(server.base_url()).unwrap();
    let meta = client.meta().unwrap();
    assert!(meta.supports_v1());
    assert!(
        server.requests()[0].header("authorization").is_none(),
        "匿名请求不应带 Authorization"
    );
}
