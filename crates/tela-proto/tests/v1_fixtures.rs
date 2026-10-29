//! 真实 API v1 的 fixture 契约：用官方 `docs/api/v1/examples/` 的响应体校验解码。
//!
//! 这些断言只依赖公开文档与示例，不依赖服务端实现细节；服务端演进（v1 只做加法）
//! 时它们应当继续通过。

use tela_proto::{
    Me, Meta, Mutation, MutationStatus, PushRequest, PushResponse, SyncResponse, TranslationState,
    Usage,
};

const SNAPSHOT: &str = include_str!("../../../tests/fixtures/api/sync-snapshot.body.json");
const DELTA: &str = include_str!("../../../tests/fixtures/api/sync-delta.body.json");
const MUTATIONS_REQUEST: &str =
    include_str!("../../../tests/fixtures/api/mutations-request.body.json");
const MUTATIONS_RESPONSE: &str = include_str!("../../../tests/fixtures/api/mutations.body.json");
const MUTATIONS_LIMIT: &str =
    include_str!("../../../tests/fixtures/api/mutations-subscription-limit.body.json");
const ME: &str = include_str!("../../../tests/fixtures/api/me.body.json");
const USAGE: &str = include_str!("../../../tests/fixtures/api/usage.body.json");
const META: &str = include_str!("../../../tests/fixtures/api/meta.body.json");
const TRANSLATION_STATUS: &str =
    include_str!("../../../tests/fixtures/api/translation-status.body.json");

#[test]
fn snapshot_has_every_default_row_type() {
    let page = SyncResponse::decode(SNAPSHOT).expect("snapshot 应可解码");
    assert!(page.reset, "首个响应应为快照");
    assert!(!page.more);
    assert_eq!(page.lang, "zh-Hans");
    assert!(page.cursor.len() > 20);
    assert!(page.removed.articles.is_empty());

    let rows = &page.rows;
    let profile = rows.profile.as_ref().expect("快照应带 profile");
    assert_eq!(profile.handle, "ann");
    assert_eq!(profile.reading_language.as_deref(), Some("zh-Hans"));
    assert!(!profile.public_likes);

    assert_eq!(rows.subscriptions.len(), 3);
    assert_eq!(rows.feeds.len(), 3);
    assert_eq!(rows.sites.len(), 3);
    assert_eq!(rows.articles.len(), 4);
    assert_eq!(rows.titles.len(), 2);
    assert_eq!(rows.states.len(), 2);
    assert_eq!(rows.recommendations.len(), 1);
    assert_eq!(rows.highlights.len(), 1);
    assert_eq!(rows.translations.len(), 2);
    assert_eq!(rows.folders.len(), 1);
    assert_eq!(rows.feed_folders.len(), 1);
    assert_eq!(rows.follows.len(), 1);
    assert_eq!(rows.prefs.len(), 1);

    // 关闭翻译的站点必须被如实解码。
    let opt_out = rows.sites.iter().find(|site| site.id == 2).expect("site 2");
    assert!(!opt_out.translation_allowed);

    // 状态字段：readAt / markedUnread / likedAt。
    let state = rows
        .states
        .iter()
        .find(|state| state.article_id == 101)
        .expect("state 101");
    assert!(state.read_at.is_some());
    assert!(state.liked_at.is_some());
    assert!(!state.marked_unread);

    // 译本的三种状态都能解。
    assert!(
        rows.translations
            .iter()
            .any(|row| row.state == TranslationState::Done)
    );
    assert!(
        rows.translations
            .iter()
            .any(|row| row.state == TranslationState::Waiting)
    );

    // 时间一律毫秒。
    assert!(rows.articles[0].fetched_at > 1_700_000_000_000);
}

#[test]
fn delta_has_null_profile_and_no_reset() {
    let page = SyncResponse::decode(DELTA).expect("delta 应可解码");
    assert!(!page.reset);
    assert!(!page.more);
    assert!(page.rows.profile.is_none(), "增量页 profile 可为 null");
    assert_eq!(page.rows.articles.len(), 1);
    assert_eq!(page.rows.articles[0].id, 103);
    assert_eq!(page.rows.titles.len(), 1);
}

#[test]
fn mutation_request_decodes_all_documented_shapes() {
    let request: PushRequest = serde_json::from_str(MUTATIONS_REQUEST).expect("请求体应可解码");
    assert_eq!(request.mutations.len(), 13);
    let types: Vec<&str> = request.mutations.iter().map(Mutation::type_name).collect();
    assert_eq!(
        types,
        [
            "subscribe",
            "subscribe",
            "subscribe",
            "putFolder",
            "fileFeed",
            "setPref",
            "setLiked",
            "markRead",
            "putHighlight",
            "recommend",
            "follow",
            "setReadingLanguage",
            "setPref"
        ]
    );

    // 21 位 mid + 13 位数字时间戳，原样保留。
    assert_eq!(request.mutations[0].mid(), "m-0001-subscribe");
    assert_eq!(request.mutations[0].at(), 1_790_596_740_000);

    // 显式 null 的字段不能丢。
    match &request.mutations[8] {
        Mutation::PutHighlight { lang, note, .. } => {
            assert!(lang.is_none());
            assert_eq!(note.as_deref(), Some("Keep this."));
        }
        other => panic!("第 9 条应为 putHighlight，实际 {other:?}"),
    }
}

#[test]
fn mutation_response_matches_by_position() {
    let response: PushResponse = serde_json::from_str(MUTATIONS_RESPONSE).expect("响应体应可解码");
    assert_eq!(response.results.len(), 13);
    assert_eq!(response.results[0].status, MutationStatus::Applied);
    assert_eq!(
        response.results[12].status,
        MutationStatus::Rejected,
        "未知 pref key 被拒"
    );
    assert_eq!(response.results[12].error.as_deref(), Some("invalid"));
}

#[test]
fn subscription_limit_is_a_rejection_not_a_http_error() {
    let response: PushResponse = serde_json::from_str(MUTATIONS_LIMIT).unwrap();
    assert_eq!(response.results.len(), 1);
    assert_eq!(response.results[0].status, MutationStatus::Rejected);
    assert_eq!(
        response.results[0].error.as_deref(),
        Some("subscription_limit")
    );
}

#[test]
fn unknown_mutation_status_is_treated_as_rejected() {
    let response: PushResponse =
        serde_json::from_str(r#"{"results":[{"mid":"m-00000001","status":"exploded"}]}"#).unwrap();
    assert_eq!(response.results[0].status, MutationStatus::Unknown);
}

#[test]
fn me_usage_meta_decode() {
    let me: Me = serde_json::from_str(ME).unwrap();
    assert_eq!(me.handle, "ann");
    assert!(me.can_write());
    assert!(me.email.is_none());

    let usage: Usage = serde_json::from_str(USAGE).unwrap();
    assert_eq!(usage.translations.per_day, Some(20));
    assert_eq!(usage.subscriptions.count, 3);
    assert_eq!(usage.rate_limits.per_member_per_minute, Some(120));

    let meta: Meta = serde_json::from_str(META).unwrap();
    assert!(meta.supports_v1());
    assert!(meta.has_feature("bodies"));
    assert_eq!(meta.limits.push_mutations, 50);
    assert_eq!(meta.limits.horizon_days, 30);
    assert_eq!(meta.limits.bodies_per_request, 25);
}

#[test]
fn translation_status_wraps_a_row() {
    let response: tela_proto::TranslationRow =
        serde_json::from_str::<serde_json::Value>(TRANSLATION_STATUS)
            .ok()
            .and_then(|value| value.get("translation").cloned())
            .and_then(|value| serde_json::from_value(value).ok())
            .expect("translation 行");
    assert_eq!(response.state, TranslationState::Requested);
    assert!(response.object.is_none());
}
