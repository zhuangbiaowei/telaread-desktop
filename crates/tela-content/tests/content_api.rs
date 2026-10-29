//! 正文/译本模型与真实 HTTP 后端：`/v1/bodies`、翻译状态、`/o/t/…`、额度拒绝。

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::MockServer;
use tela_content::api::{ChapterRequest, ContentApi};
use tela_content::http::HttpContentApi;
use tela_content::model::{ContentObject, TranslationObject, TranslationState, TranslationView};
use tela_net::{ApiClient, StaticToken};
use tela_proto::TelaError;

const CONTENT: &str = include_str!("../../../tests/fixtures/content/content-normal.json");
const TRANSLATION: &str =
    include_str!("../../../tests/fixtures/content/translation-normal.zh-Hans.json");
const KEY: &str = "11111111111111111111111111111111";

fn bodies_response(extra: &str) -> String {
    format!(r#"{{"objects":{{"{KEY}":{CONTENT}}},"refused":{{{extra}}}}}"#)
}

fn api(server: &MockServer) -> Arc<ApiClient> {
    Arc::new(
        ApiClient::with_timeout(
            server.base_url(),
            Arc::new(StaticToken("tela_at_test".into())),
            Duration::from_secs(5),
        )
        .unwrap(),
    )
}

#[test]
fn content_object_fixture_pages_and_alignment() {
    let content: ContentObject = serde_json::from_str(CONTENT).unwrap();
    assert_eq!(content.total_blocks(), 5);
    let page = content.page(1, 2);
    assert_eq!(page.blocks.len(), 2);
    assert_eq!(page.offset, 1);
    assert_eq!(page.total, 5);
    assert!(page.has_more);
    assert!(content.page(99, 10).blocks.is_empty());

    let translation: TranslationObject = serde_json::from_str(TRANSLATION).unwrap();
    assert_eq!(translation.source, KEY);
    assert_eq!(translation.status, "done");
    let aligned = translation.align(content.total_blocks());
    assert_eq!(aligned.len(), 5);
    assert!(aligned.iter().all(Option::is_some));
    assert!(!translation.is_partial());
}

#[test]
fn loads_a_chapter_with_a_finished_translation() {
    let status = format!(
        r#"{{"translation":{{"contentKey":"{KEY}","lang":"zh-Hans","state":"done",
            "chunks":[],"object":"/o/t/{KEY}/zh-Hans/v1.json","dueAt":null}}}}"#
    );
    let server = MockServer::start(3, move |request| match request.path.as_str() {
        path if path.starts_with("/v1/bodies") => (200, bodies_response("")),
        path if path.starts_with("/v1/translations/") => (200, status.clone()),
        path if path.starts_with("/o/t/") => (200, TRANSLATION.to_string()),
        other => (404, format!(r#"{{"error":"not_found","path":"{other}"}}"#)),
    });

    let api = HttpContentApi::new(api(&server));
    let chapter = api
        .load_chapter(&ChapterRequest::new(KEY, "zh-Hans"))
        .unwrap();

    assert_eq!(chapter.total_blocks(), 5);
    assert!(matches!(chapter.translation, TranslationView::Ready { .. }));
    assert!(!chapter.translation.has_gaps());

    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].path.starts_with("/v1/bodies?keys="));
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer tela_at_test")
    );
    assert_eq!(requests[2].path, format!("/o/t/{KEY}/zh-Hans/v1.json"));
}

#[test]
fn a_refused_body_is_a_daily_limit_error() {
    let refused =
        format!(r#""{KEY}":{{"error":"post_limit","limit":100,"resetsAt":1761609600000}}"#);
    let body = format!(r#"{{"objects":{{}},"refused":{{{refused}}}}}"#);
    let server = MockServer::start(1, move |_| (200, body.clone()));

    let api = HttpContentApi::new(api(&server));
    let error = api
        .load_chapter(&ChapterRequest::new(KEY, "zh-Hans"))
        .unwrap_err();
    match error {
        TelaError::Limit {
            code,
            limit,
            resets_at,
            ..
        } => {
            assert_eq!(code, "post_limit");
            assert_eq!(limit, Some(100));
            assert_eq!(resets_at, Some(1_761_609_600_000));
        }
        other => panic!("应为每日额度错误，实际 {other:?}"),
    }
}

#[test]
fn a_running_translation_is_pending_not_an_error() {
    let status = format!(
        r#"{{"translation":{{"contentKey":"{KEY}","lang":"zh-Hans","state":"running",
            "chunks":["/o/tc/x/0.json"],"object":null,"dueAt":null}}}}"#
    );
    let server = MockServer::start(2, move |request| {
        if request.path.starts_with("/v1/bodies") {
            (200, bodies_response(""))
        } else {
            (200, status.clone())
        }
    });
    let api = HttpContentApi::new(api(&server));
    let chapter = api
        .load_chapter(&ChapterRequest::new(KEY, "zh-Hans"))
        .unwrap();
    match chapter.translation {
        TranslationView::Pending { state, .. } => {
            assert_eq!(state, TranslationState::Running);
        }
        other => panic!("应为 Pending，实际 {other:?}"),
    }
}

#[test]
fn no_translation_row_means_not_available() {
    let server = MockServer::start(2, move |request| {
        if request.path.starts_with("/v1/bodies") {
            (200, bodies_response(""))
        } else {
            (200, r#"{"translation":null}"#.to_string())
        }
    });
    let api = HttpContentApi::new(api(&server));
    let chapter = api
        .load_chapter(&ChapterRequest::new(KEY, "zh-Hans"))
        .unwrap();
    assert_eq!(chapter.translation, TranslationView::NotAvailable);
}

#[test]
fn translation_requests_post_article_and_language() {
    let server = MockServer::start(1, |_| {
        (
            200,
            format!(
                r#"{{"status":"running","translation":{{"contentKey":"{KEY}",
                "lang":"zh-Hans","state":"requested","chunks":[],"object":null,"dueAt":null}}}}"#
            ),
        )
    });
    let api = HttpContentApi::new(api(&server));
    let answer = api.request_translation(401, "zh-Hans").unwrap();
    assert_eq!(
        answer.status,
        tela_content::model::TranslationAnswerStatus::Running
    );
    let request = &server.requests()[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/translations");
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["articleId"], 401);
    assert_eq!(body["lang"], "zh-Hans");
}
