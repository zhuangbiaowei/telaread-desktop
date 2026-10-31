//! schema v4 与真实同步语义：快照/增量、删除级联、离线队列、正文/译文/图片缓存、旧库迁移。

use tela_proto::{
    ArticleRow, FeedRow, Mutation, Removed, SiteRow, StateRow, SubscriptionRow, SyncResponse,
    SyncRows, TitleRow, TranslationRow, TranslationState,
};
use tela_store::{LATEST_SCHEMA_VERSION, Store};

const SNAPSHOT: &str = include_str!("../../../tests/fixtures/api/sync-snapshot.body.json");
const DELTA: &str = include_str!("../../../tests/fixtures/api/sync-delta.body.json");

fn article(id: i64, feed_id: i64, content_key: Option<&str>) -> ArticleRow {
    ArticleRow {
        id,
        feed_id,
        url: Some(format!("https://example.com/{id}")),
        title: format!("article {id}"),
        author: None,
        published_at: Some(1_790_000_000_000),
        fetched_at: 1_790_000_000_000,
        sort_at: 1_790_000_000_000,
        source_lang: Some("en".into()),
        excerpt: None,
        content_key: content_key.map(str::to_string),
        word_count: 10,
        reading_minutes: 1,
        like_count: 0,
        recommend_count: 0,
    }
}

fn sync(rows: SyncRows, reset: bool, cursor: &str) -> SyncResponse {
    SyncResponse {
        cursor: cursor.to_string(),
        more: false,
        reset,
        lang: "zh-Hans".into(),
        types: tela_proto::DEFAULT_SYNC_TYPES
            .iter()
            .map(|name| name.to_string())
            .collect(),
        removed: Removed::default(),
        rows,
    }
}

fn store() -> Store {
    Store::open_in_memory().expect("open in-memory store")
}

#[test]
fn snapshot_populates_rows_and_opaque_cursor() {
    let mut store = store();
    let page = SyncResponse::decode(SNAPSHOT).unwrap();
    let outcome = store.apply_sync(&page).unwrap();
    assert!(outcome.reset);
    assert!(
        outcome.rows >= 25,
        "快照应写入全部行，实际 {}",
        outcome.rows
    );

    assert_eq!(store.row_count("articles").unwrap(), 4);
    assert_eq!(store.row_count("feeds").unwrap(), 3);
    assert_eq!(store.row_count("sites").unwrap(), 3);
    assert_eq!(
        store.cursor().unwrap().as_deref(),
        Some(page.cursor.as_str())
    );
    assert_eq!(store.sync_lang().unwrap().as_deref(), Some("zh-Hans"));
    assert_eq!(store.sync_types().unwrap().len(), 14);

    let profile: tela_proto::ProfileRow = store.get_as("profile", "profile").unwrap().unwrap();
    assert_eq!(profile.handle, "ann");
}

#[test]
fn delta_upserts_and_keeps_previous_rows() {
    let mut store = store();
    store
        .apply_sync(&SyncResponse::decode(SNAPSHOT).unwrap())
        .unwrap();
    let delta = SyncResponse::decode(DELTA).unwrap();
    store.apply_sync(&delta).unwrap();

    assert_eq!(store.row_count("articles").unwrap(), 5);
    assert_eq!(
        store.cursor().unwrap().as_deref(),
        Some(delta.cursor.as_str())
    );
    // profile 在增量页是 null，不能被清掉。
    let profile: tela_proto::ProfileRow = store.get_as("profile", "profile").unwrap().unwrap();
    assert_eq!(profile.handle, "ann");
    assert!(
        store
            .get_as::<ArticleRow>("articles", "103")
            .unwrap()
            .is_some()
    );
}

#[test]
fn reset_discards_rows_not_in_the_new_snapshot() {
    let mut store = store();
    store
        .apply_sync(&SyncResponse::decode(SNAPSHOT).unwrap())
        .unwrap();

    let mut rows = SyncRows::default();
    rows.articles.push(article(900, 1, None));
    store.apply_sync(&sync(rows, true, "snapshot-2")).unwrap();

    assert_eq!(store.row_count("articles").unwrap(), 1);
    assert_eq!(store.row_count("feeds").unwrap(), 0);
    assert!(
        store
            .get_as::<tela_proto::ProfileRow>("profile", "profile")
            .unwrap()
            .is_none()
    );
}

#[test]
fn removed_articles_take_their_derived_rows_with_them() {
    let mut store = store();
    let mut rows = SyncRows::default();
    rows.articles
        .push(article(7, 1, Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")));
    rows.states.push(StateRow {
        article_id: 7,
        read_at: Some(1),
        marked_unread: false,
        liked_at: None,
    });
    rows.titles.push(TitleRow {
        article_id: 7,
        lang: "zh-Hans".into(),
        title: Some("七".into()),
        excerpt: None,
    });
    rows.translations.push(TranslationRow {
        content_key: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        lang: "zh-Hans".into(),
        state: TranslationState::Done,
        chunks: vec![],
        object: Some("/o/t/x".into()),
        due_at: None,
    });
    store.apply_sync(&sync(rows, true, "c1")).unwrap();

    let mut page = sync(SyncRows::default(), false, "c2");
    page.removed.articles = vec![7];
    store.apply_sync(&page).unwrap();

    assert!(
        store
            .get_as::<ArticleRow>("articles", "7")
            .unwrap()
            .is_none()
    );
    assert!(store.get_as::<StateRow>("states", "7").unwrap().is_none());
    assert!(
        store
            .get_as::<TitleRow>("titles", "7/zh-Hans")
            .unwrap()
            .is_none()
    );
    assert_eq!(store.row_count("translations").unwrap(), 0);
}

#[test]
fn translation_opt_out_drops_titles_and_translations() {
    let mut store = store();
    let mut rows = SyncRows::default();
    rows.sites.push(SiteRow {
        id: 1,
        home_url: "https://example.com".into(),
        title: Some("Blog".into()),
        description: None,
        favicon_url: None,
        primary_lang: Some("en".into()),
        reader_count: 0,
        translation_allowed: false,
        claimed: false,
    });
    rows.feeds.push(FeedRow {
        id: 10,
        site_id: 1,
        feed_url: "https://example.com/feed.xml".into(),
        title: None,
        status: "active".into(),
        last_fetched_at: None,
        last_item_at: None,
    });
    rows.articles
        .push(article(11, 10, Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")));
    rows.titles.push(TitleRow {
        article_id: 11,
        lang: "zh-Hans".into(),
        title: Some("译名".into()),
        excerpt: None,
    });
    rows.translations.push(TranslationRow {
        content_key: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        lang: "zh-Hans".into(),
        state: TranslationState::Done,
        chunks: vec![],
        object: None,
        due_at: None,
    });

    store.apply_sync(&sync(rows, true, "c1")).unwrap();
    assert_eq!(store.row_count("titles").unwrap(), 0);
    assert_eq!(store.row_count("translations").unwrap(), 0);
    // 文章本身仍然保留，只是按原文显示。
    assert!(
        store
            .get_as::<ArticleRow>("articles", "11")
            .unwrap()
            .is_some()
    );
}

#[test]
fn unsubscribing_removes_posts_but_keeps_liked_ones() {
    let mut store = store();
    let mut rows = SyncRows::default();
    rows.subscriptions.push(SubscriptionRow {
        feed_id: 3,
        watermark_id: 0,
        created_at: 1,
        deleted_at: None,
    });
    rows.articles.push(article(31, 3, None));
    rows.articles.push(article(32, 3, None));
    rows.states.push(StateRow {
        article_id: 31,
        read_at: None,
        marked_unread: false,
        liked_at: Some(5),
    });
    store.apply_sync(&sync(rows, true, "c1")).unwrap();
    assert_eq!(store.row_count("articles").unwrap(), 2);

    let mut rows = SyncRows::default();
    rows.subscriptions.push(SubscriptionRow {
        feed_id: 3,
        watermark_id: 0,
        created_at: 1,
        deleted_at: Some(9),
    });
    store.apply_sync(&sync(rows, false, "c2")).unwrap();

    assert!(
        store
            .get_as::<ArticleRow>("articles", "31")
            .unwrap()
            .is_some(),
        "被点赞的文章要留下"
    );
    assert!(
        store
            .get_as::<ArticleRow>("articles", "32")
            .unwrap()
            .is_none()
    );
}

#[test]
fn pending_queue_is_fifo_and_idempotent() {
    let mut store = store();
    let first = Mutation::mark_read(1);
    let second = Mutation::mark_unread(2);
    assert!(store.enqueue_mutation(&first).unwrap());
    assert!(store.enqueue_mutation(&second).unwrap());
    assert!(
        !store.enqueue_mutation(&first).unwrap(),
        "同一个 mid 重复入队应被忽略"
    );
    assert_eq!(store.pending_count().unwrap(), 2);

    let pending = store.pending_mutations(10).unwrap();
    assert_eq!(pending[0].mid(), first.mid());
    assert_eq!(pending[1].mid(), second.mid());

    store.delete_mutations(&[first.mid().to_string()]).unwrap();
    assert_eq!(store.pending_count().unwrap(), 1);
    assert_eq!(store.pending_mutations(10).unwrap()[0].mid(), second.mid());
}

#[test]
fn cursor_language_and_types_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tela.db");
    {
        let mut store = Store::open(&path).unwrap();
        store
            .set_sync_state("eyJ2IjoxLCJzIjoyMH0", "zh-Hans", &["profile".to_string()])
            .unwrap();
        assert_eq!(store.schema_version(), LATEST_SCHEMA_VERSION);
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.cursor().unwrap().as_deref(),
        Some("eyJ2IjoxLCJzIjoyMH0")
    );
    assert_eq!(store.sync_lang().unwrap().as_deref(), Some("zh-Hans"));
    assert_eq!(store.sync_types().unwrap(), ["profile"]);
}

#[test]
fn body_cache_round_trips() {
    let mut store = store();
    assert!(
        store
            .get_body("cccccccccccccccccccccccccccccccc")
            .unwrap()
            .is_none()
    );
    store
        .put_body("cccccccccccccccccccccccccccccccc", r#"{"format":1}"#)
        .unwrap();
    assert_eq!(
        store
            .get_body("cccccccccccccccccccccccccccccccc")
            .unwrap()
            .as_deref(),
        Some(r#"{"format":1}"#)
    );
    assert_eq!(store.body_count().unwrap(), 1);
}

#[test]
fn translation_cache_round_trips_and_is_kept_on_clear_account() {
    let mut store = store();
    assert!(
        store
            .get_translation("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", "zh-Hans")
            .unwrap()
            .is_none()
    );
    store
        .put_translation(
            "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            "zh-Hans",
            "done",
            Some(r#"{"format":1,"lang":"zh-Hans","status":"done"}"#),
        )
        .unwrap();
    let cached = store
        .get_translation("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", "zh-Hans")
        .unwrap()
        .expect("译文快照应已写入");
    assert_eq!(cached.state, "done");
    assert!(cached.object.as_deref().unwrap().contains("\"done\""));

    // pending → done 覆盖写，不产生第二行。
    store
        .put_translation(
            "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            "zh-Hans",
            "partial",
            None,
        )
        .unwrap();
    assert_eq!(
        store
            .get_translation("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", "zh-Hans")
            .unwrap()
            .unwrap()
            .state,
        "partial"
    );
    assert_eq!(store.translation_count().unwrap(), 1);
    assert_eq!(
        store
            .translation_count_for("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
            .unwrap(),
        1
    );

    store.clear_account().unwrap();
    assert_eq!(store.translation_count().unwrap(), 1, "登出保留离线缓存");
}

#[test]
fn image_cache_round_trips_and_is_kept_on_clear_account() {
    let mut store = store();
    assert!(store.get_image("/img/abc/1").unwrap().is_none());
    store
        .put_image("/img/abc/1", &[0x89, b'P', b'N', b'G', 1, 2, 3])
        .unwrap();
    assert_eq!(
        store.get_image("/img/abc/1").unwrap().as_deref(),
        Some(&[0x89, b'P', b'N', b'G', 1, 2, 3][..])
    );
    // 覆盖写不产生第二行。
    store.put_image("/img/abc/1", b"new-bytes").unwrap();
    assert_eq!(
        store.get_image("/img/abc/1").unwrap().as_deref(),
        Some(&b"new-bytes"[..])
    );
    assert_eq!(store.image_count().unwrap(), 1);

    store.clear_account().unwrap();
    assert_eq!(store.image_count().unwrap(), 1, "登出保留离线图片");
}

#[test]
fn v2_database_is_upgraded_without_losing_body_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v2.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
             INSERT INTO meta (key, value) VALUES ('schema_version', 2);
             CREATE TABLE sync_rows (entity TEXT NOT NULL, row_key TEXT NOT NULL,
                                     body TEXT NOT NULL, PRIMARY KEY (entity, row_key));
             INSERT INTO sync_rows VALUES ('articles', '1', '{}');
             CREATE TABLE pending_mutations (seq INTEGER PRIMARY KEY AUTOINCREMENT,
                                             mid TEXT NOT NULL UNIQUE, at INTEGER NOT NULL,
                                             body TEXT NOT NULL);
             CREATE TABLE bodies (content_key TEXT PRIMARY KEY NOT NULL,
                                  body TEXT NOT NULL, fetched_at INTEGER NOT NULL);
             INSERT INTO bodies VALUES ('ffffffffffffffffffffffffffffffff', '{}', 0);",
        )
        .unwrap();
    }
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version(), LATEST_SCHEMA_VERSION);
    assert_eq!(
        store
            .get_body("ffffffffffffffffffffffffffffffff")
            .unwrap()
            .as_deref(),
        Some("{}"),
        "升级只补新表，不丢正文缓存"
    );
    // 新表可用。
    store
        .put_translation("ffffffffffffffffffffffffffffffff", "zh-Hans", "none", None)
        .unwrap();
    assert_eq!(store.translation_count().unwrap(), 1);
    store.put_image("/img/ffff/1", b"png").unwrap();
    assert_eq!(
        store.get_image("/img/ffff/1").unwrap().as_deref(),
        Some(&b"png"[..])
    );
}

#[test]
fn v1_mock_database_is_migrated_to_latest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY NOT NULL, value INTEGER NOT NULL);
             INSERT INTO meta (key, value) VALUES ('schema_version', 1);
             CREATE TABLE sync_rows (entity TEXT NOT NULL, row_key TEXT NOT NULL,
                                     seq INTEGER NOT NULL, body TEXT NOT NULL,
                                     PRIMARY KEY (entity, row_key));
             INSERT INTO sync_rows VALUES ('articles', '1', 1, '{}');",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version(), LATEST_SCHEMA_VERSION);
    assert_eq!(
        store.row_count("articles").unwrap(),
        0,
        "旧 mock 行必须被丢弃"
    );
    assert!(store.cursor().unwrap().is_none());
}

#[test]
fn clear_account_keeps_bodies() {
    let mut store = store();
    store
        .apply_sync(&SyncResponse::decode(SNAPSHOT).unwrap())
        .unwrap();
    store
        .put_body("dddddddddddddddddddddddddddddddd", "{}")
        .unwrap();
    store.enqueue_mutation(&Mutation::mark_read(1)).unwrap();

    store.clear_account().unwrap();
    assert_eq!(store.row_count("articles").unwrap(), 0);
    assert_eq!(store.pending_count().unwrap(), 0);
    assert!(store.cursor().unwrap().is_none());
    assert_eq!(store.body_count().unwrap(), 1);
}
