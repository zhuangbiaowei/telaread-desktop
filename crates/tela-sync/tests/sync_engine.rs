//! 同步编排：快照/续拉、语言切换重快照、重试边界、推送队列与顺序。

use std::time::Duration;

use tela_net::{MockTransport, TransportCall};
use tela_proto::{
    ArticleRow, Mutation, MutationResult, MutationStatus, PushResponse, Removed, SyncResponse,
    SyncRows, TelaError, WireContext,
};
use tela_store::Store;
use tela_sync::{PullOptions, PushOptions, SyncEngine};

fn article(id: i64) -> ArticleRow {
    ArticleRow {
        id,
        feed_id: 1,
        url: None,
        title: format!("article {id}"),
        author: None,
        published_at: None,
        fetched_at: 1_790_000_000_000,
        sort_at: 1_790_000_000_000,
        source_lang: None,
        excerpt: None,
        content_key: None,
        word_count: 0,
        reading_minutes: 0,
        like_count: 0,
        recommend_count: 0,
    }
}

fn page(
    cursor: &str,
    lang: &str,
    reset: bool,
    more: bool,
    articles: Vec<ArticleRow>,
) -> SyncResponse {
    SyncResponse {
        cursor: cursor.to_string(),
        more,
        reset,
        lang: lang.to_string(),
        types: tela_proto::DEFAULT_SYNC_TYPES
            .iter()
            .map(|name| name.to_string())
            .collect(),
        removed: Removed::default(),
        rows: SyncRows {
            articles,
            ..SyncRows::default()
        },
    }
}

fn result(mid: &str, status: MutationStatus, error: Option<&str>) -> MutationResult {
    MutationResult {
        mid: Some(mid.to_string()),
        status,
        error: error.map(str::to_string),
    }
}

fn options(lang: &str) -> PullOptions {
    PullOptions::new(lang)
        .with_retry_delay(Duration::ZERO)
        .with_max_attempts(3)
}

#[test]
fn more_pages_are_pulled_in_order_with_the_new_cursor() {
    let mut store = Store::open_in_memory().unwrap();
    let transport = MockTransport::new();
    transport.script_pull(page("c1", "zh-Hans", true, true, vec![article(1)]));
    transport.script_pull(page("c2", "zh-Hans", false, false, vec![article(2)]));

    let mut engine = SyncEngine::new(&mut store, &transport);
    let report = engine.pull(&options("zh-Hans")).unwrap();

    assert_eq!(report.pages, 2);
    assert_eq!(report.rows, 2);
    assert!(report.reset);
    assert_eq!(report.cursor.as_deref(), Some("c2"));
    assert_eq!(store.row_count("articles").unwrap(), 2);

    let calls = transport.calls();
    assert_eq!(calls.len(), 2);
    match (&calls[0], &calls[1]) {
        (TransportCall::Pull(first), TransportCall::Pull(second)) => {
            assert!(first.cursor.is_none());
            assert_eq!(first.lang, "zh-Hans");
            assert_eq!(first.types.len(), 14, "默认发送 v1 的 14 张表");
            assert_eq!(second.cursor.as_deref(), Some("c1"));
        }
        other => panic!("调用顺序不对：{other:?}"),
    }
}

#[test]
fn changing_language_forces_a_snapshot() {
    let mut store = Store::open_in_memory().unwrap();
    let first = MockTransport::new();
    first.script_pull(page("en-1", "en", true, false, vec![article(1)]));
    SyncEngine::new(&mut store, &first)
        .pull(&options("en"))
        .unwrap();
    assert_eq!(store.sync_lang().unwrap().as_deref(), Some("en"));

    let second = MockTransport::new();
    second.script_pull(page("zh-1", "zh-Hans", true, false, vec![article(2)]));
    let mut engine = SyncEngine::new(&mut store, &second);
    engine.pull(&options("zh-Hans")).unwrap();

    match &second.calls()[0] {
        TransportCall::Pull(query) => assert!(query.cursor.is_none(), "换语言必须从快照开始"),
        other => panic!("{other:?}"),
    }
    assert_eq!(store.row_count("articles").unwrap(), 1, "快照会替换旧行");
}

#[test]
fn limiting_types_forces_a_snapshot_when_the_cursor_did_not_cover_them() {
    let mut store = Store::open_in_memory().unwrap();
    let first = MockTransport::new();
    first.script_pull(page("c1", "zh-Hans", true, false, vec![]));
    SyncEngine::new(&mut store, &first)
        .pull(&options("zh-Hans"))
        .unwrap();

    let second = MockTransport::new();
    second.script_pull(page("c2", "zh-Hans", true, false, vec![]));
    let mut engine = SyncEngine::new(&mut store, &second);
    engine
        .pull(&options("zh-Hans").with_types(["articles", "states"]))
        .unwrap();

    match &second.calls()[0] {
        TransportCall::Pull(query) => {
            assert!(query.cursor.is_none());
            assert_eq!(query.types, ["articles", "states"]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn transient_failures_are_retried_on_the_same_cursor() {
    let mut store = Store::open_in_memory().unwrap();
    let transport = MockTransport::new();
    transport.script_pull_error(TelaError::Net {
        ctx: WireContext::new("mock://sync", "sync"),
        detail: "503".into(),
    });
    transport.script_pull(page("c1", "zh-Hans", true, false, vec![article(1)]));

    let mut engine = SyncEngine::new(&mut store, &transport);
    let report = engine.pull(&options("zh-Hans")).unwrap();
    assert_eq!(report.attempts, 2);
    assert_eq!(report.pages, 1);
}

#[test]
fn auth_errors_are_not_retried() {
    let mut store = Store::open_in_memory().unwrap();
    let transport = MockTransport::new();
    transport.script_pull_error(TelaError::Auth {
        ctx: WireContext::new("mock://sync", "sync"),
        detail: "invalid_token".into(),
    });

    let mut engine = SyncEngine::new(&mut store, &transport);
    let error = engine.pull(&options("zh-Hans")).unwrap_err();
    assert_eq!(error.exit_code(), 21);
    assert_eq!(transport.calls().len(), 1, "401/403 不得重试");
}

#[test]
fn push_drops_every_answered_mutation_and_reports_rejections() {
    let mut store = Store::open_in_memory().unwrap();
    let mutations = vec![
        Mutation::mark_read(1),
        Mutation::set_liked(2, true),
        Mutation::mark_unread(3),
    ];
    for mutation in &mutations {
        store.enqueue_mutation(mutation).unwrap();
    }

    let transport = MockTransport::new();
    transport.script_push(PushResponse {
        results: vec![
            result(mutations[0].mid(), MutationStatus::Applied, None),
            result(
                mutations[1].mid(),
                MutationStatus::Rejected,
                Some("subscription_limit"),
            ),
            result(mutations[2].mid(), MutationStatus::Applied, None),
        ],
    });

    let mut engine = SyncEngine::new(&mut store, &transport);
    let report = engine.push(&PushOptions::default()).unwrap();

    assert_eq!(report.sent, 3);
    assert_eq!(report.applied, 2);
    assert_eq!(report.rejected.len(), 1);
    assert_eq!(report.rejected[0].error, "subscription_limit");
    assert_eq!(report.remaining, 0);
    assert_eq!(store.pending_count().unwrap(), 0, "被拒的也要从队列丢弃");
}

#[test]
fn push_failure_keeps_the_queue_unchanged() {
    let mut store = Store::open_in_memory().unwrap();
    store.enqueue_mutation(&Mutation::mark_read(1)).unwrap();
    store.enqueue_mutation(&Mutation::mark_read(2)).unwrap();

    let transport = MockTransport::new();
    transport.script_push_error(TelaError::Net {
        ctx: WireContext::new("mock://mutations", "mutations"),
        detail: "offline".into(),
    });

    let mut engine = SyncEngine::new(&mut store, &transport);
    let error = engine.push(&PushOptions::default()).unwrap_err();
    assert!(error.is_retryable());
    assert_eq!(error.report.remaining, 2, "失败报告也要带剩余条数");
    assert_eq!(store.pending_count().unwrap(), 2);
}

#[test]
fn a_short_result_list_is_a_protocol_error_and_keeps_the_queue() {
    let mut store = Store::open_in_memory().unwrap();
    let mutation = Mutation::mark_read(1);
    store.enqueue_mutation(&mutation).unwrap();

    let transport = MockTransport::new();
    transport.script_push(PushResponse { results: vec![] });

    let mut engine = SyncEngine::new(&mut store, &transport);
    let error = engine.push(&PushOptions::default()).unwrap_err();
    assert_eq!(error.source.code(), "protocol");
    assert_eq!(store.pending_count().unwrap(), 1);
}

#[test]
fn a_mismatched_mid_is_a_protocol_error() {
    let mut store = Store::open_in_memory().unwrap();
    let mutation = Mutation::mark_read(1);
    store.enqueue_mutation(&mutation).unwrap();

    let transport = MockTransport::new();
    transport.script_push(PushResponse {
        results: vec![result("m-other-0001", MutationStatus::Applied, None)],
    });

    let mut engine = SyncEngine::new(&mut store, &transport);
    let error = engine.push(&PushOptions::default()).unwrap_err();
    assert_eq!(error.source.code(), "protocol");
    assert_eq!(store.pending_count().unwrap(), 1);
}

#[test]
fn sync_pushes_before_it_pulls() {
    let mut store = Store::open_in_memory().unwrap();
    let mutation = Mutation::mark_read(1);
    store.enqueue_mutation(&mutation).unwrap();

    let transport = MockTransport::new();
    transport.script_push(PushResponse {
        results: vec![result(mutation.mid(), MutationStatus::Applied, None)],
    });
    transport.script_pull(page("c1", "zh-Hans", true, false, vec![article(1)]));

    let mut engine = SyncEngine::new(&mut store, &transport);
    let report = engine.sync(&options("zh-Hans")).unwrap();

    assert_eq!(report.pull.pages, 1);
    assert_eq!(report.push.unwrap().applied, 1);
    assert!(matches!(transport.calls()[0], TransportCall::Push(_)));
    assert!(matches!(transport.calls()[1], TransportCall::Pull(_)));
}
