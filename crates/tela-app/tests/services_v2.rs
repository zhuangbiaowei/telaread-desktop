//! 装配层（真实协议 v2）：演示 seed、译文标题、阅读状态、正文缓存、重开库。

use std::sync::Arc;

use tela_app::services::{
    AppConfig, AppServices, DEMO_ACCOUNT, DEMO_ARTICLE_LONG, DEMO_ARTICLE_NORMAL,
    DEMO_ARTICLE_READ, DEMO_FEED_ID, DEMO_FEED_ID_2,
};
use tela_content::api::ContentApi;
use tela_content::mock::{MOCK_CONTENT_KEY, MOCK_LONG_CONTENT_KEY, MockContentApi};
use tela_content::model::TranslationView;
use tela_net::MockTransport;
use tela_proto::TelaError;
use tela_ui::account::AccountMode;
use tela_ui::shell::ShellData;

fn content() -> Arc<dyn ContentApi> {
    Arc::new(MockContentApi::new())
}

fn demo_services() -> AppServices {
    let services =
        AppServices::open_in_memory("zh-Hans", content(), Arc::new(MockTransport::new()))
            .expect("open in-memory services");
    services.seed_demo().expect("seed demo");
    services
}

#[test]
fn empty_database_is_empty() {
    let services =
        AppServices::open_in_memory("zh-Hans", content(), Arc::new(MockTransport::new())).unwrap();
    assert!(services.list_feeds().unwrap().is_empty());
    assert!(services.list_articles(DEMO_FEED_ID).unwrap().is_empty());
}

#[test]
fn seed_is_idempotent_and_reports_unread_by_the_five_rules() {
    let services = demo_services();
    assert!(!services.seed_demo().unwrap(), "已有数据时不再 seed");

    let feeds = services.list_feeds().unwrap();
    assert_eq!(feeds.len(), 2);
    assert_eq!(feeds[0].id, DEMO_FEED_ID);
    assert_eq!(feeds[0].title, "Tela 示例博客");
    assert_eq!(feeds[0].article_count, 3);
    assert_eq!(feeds[0].unread, 3);
    assert_eq!(feeds[1].id, DEMO_FEED_ID_2);
    assert_eq!(feeds[1].unread, 0, "state.readAt 让文章 45 已读");

    let articles = services.list_articles(DEMO_FEED_ID).unwrap();
    assert_eq!(articles.len(), 3);
    assert_eq!(articles[0].id, DEMO_ARTICLE_NORMAL);
    assert_eq!(
        articles[0].title, "你好，Tela：完整对照",
        "标题取当前语言译文"
    );
    assert_eq!(
        articles[0].url.as_deref(),
        Some(format!("https://example.com/articles/{DEMO_ARTICLE_NORMAL}").as_str()),
        "阅读面板的“打开原文”依赖这个 URL"
    );
    assert!(articles.iter().all(|article| article.unread));
}

#[test]
fn chapter_requests_use_the_content_key_and_language() {
    let services = demo_services();
    let request = services.chapter_request_for(DEMO_ARTICLE_NORMAL).unwrap();
    assert_eq!(request.content_key, MOCK_CONTENT_KEY);
    assert_eq!(request.lang, "zh-Hans");
    assert_eq!(
        services
            .chapter_request_for(DEMO_ARTICLE_LONG)
            .unwrap()
            .content_key,
        MOCK_LONG_CONTENT_KEY
    );

    let chapter = services.load_chapter(DEMO_ARTICLE_NORMAL).unwrap();
    assert_eq!(chapter.total_blocks(), 5);
    assert!(matches!(chapter.translation, TranslationView::Ready { .. }));
}

#[test]
fn reading_a_post_warms_the_body_cache() {
    let services = demo_services();
    services.load_chapter(DEMO_ARTICLE_NORMAL).unwrap();
    let status = services.sync_status().unwrap();
    assert_eq!(status.bodies, 1, "正文对象应缓存到本地库");
}

#[test]
fn demo_seed_survives_reopening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tela.db");

    {
        let config = AppConfig::for_tests(&path);
        let services =
            AppServices::open(config, content(), Arc::new(MockTransport::new())).unwrap();
        assert!(services.seed_demo().unwrap());
    }
    {
        let config = AppConfig::for_tests(&path);
        let services =
            AppServices::open(config, content(), Arc::new(MockTransport::new())).unwrap();
        assert_eq!(services.list_feeds().unwrap().len(), 2);
        assert!(services.list_articles(DEMO_FEED_ID).unwrap().len() == 3);
        assert!(services.list_articles(DEMO_FEED_ID_2).unwrap()[0].id == DEMO_ARTICLE_READ);
    }
}

#[test]
fn shell_data_trait_exports_the_same_view() {
    let services = demo_services();
    let data: &dyn ShellData = &services;
    assert_eq!(data.feeds().unwrap().len(), 2);
    assert_eq!(data.articles(DEMO_FEED_ID).unwrap().len(), 3);
    assert_eq!(
        data.chapter_request(DEMO_ARTICLE_NORMAL)
            .unwrap()
            .content_key,
        MOCK_CONTENT_KEY
    );
}

#[test]
fn demo_seed_marks_the_database_as_demo() {
    let services = demo_services();
    assert_eq!(
        services.sync_status().unwrap().account.as_deref(),
        Some(DEMO_ACCOUNT),
        "真实装配靠这个标记识别并清掉 demo 行"
    );
}

#[test]
fn real_bootstrap_clears_a_demo_database_before_syncing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tela.db");
    {
        let services = AppServices::open(
            AppConfig::for_tests(&path),
            content(),
            Arc::new(MockTransport::new()),
        )
        .unwrap();
        assert!(services.seed_demo().unwrap());
        assert_eq!(
            services.sync_status().unwrap().account.as_deref(),
            Some(DEMO_ACCOUNT)
        );
    }

    // 同一个文件切到真实装配：demo 行、demo 游标与 demo 账号都必须消失。
    let services = AppServices::bootstrap_real_with(AppConfig::for_tests(&path)).unwrap();
    let status = services.sync_status().unwrap();
    assert_eq!(status.total_rows(), 0);
    assert!(status.cursor.is_none());
    assert!(status.account.is_none(), "没有令牌时不能保留 demo 账号");
}

#[test]
fn gui_login_ticket_has_pkce_url_and_can_be_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let config = AppConfig::for_tests(dir.path().join("tela.db"));
    let services = AppServices::bootstrap_real_with(config).unwrap();

    let status = services.account_status();
    assert_eq!(status.mode, AccountMode::Real);
    assert!(!status.connected);
    assert!(status.client_id_configured);

    let ticket = services.begin_login().unwrap();
    assert!(
        ticket.auth_url.contains("client_id=tela_app_test"),
        "{}",
        ticket.auth_url
    );
    assert!(ticket.auth_url.contains("code_challenge_method=S256"));
    assert!(
        ticket
            .auth_url
            .contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A")
    );

    services.cancel_login();
    assert!(
        services.wait_login(&ticket).is_err(),
        "取消后旧票据必须失效"
    );
}

#[test]
fn demo_services_expose_account_mode_but_refuse_login() {
    let services = demo_services();
    assert_eq!(services.account_status().mode, AccountMode::Demo);
    assert!(!services.account_status().can_login());
    assert!(services.begin_login().is_err());
}

#[test]
fn demo_seed_exposes_a_recommendation_through_shell_data() {
    let services = demo_services();
    let recommendations = services.list_recommendations().unwrap();
    assert_eq!(recommendations.len(), 1);
    let item = &recommendations[0];
    assert_eq!(item.article_id, DEMO_ARTICLE_NORMAL);
    assert_eq!(
        item.title, "你好，Tela：完整对照",
        "推荐列表标题取当前语言译文"
    );
    assert_eq!(item.feed_title, "Tela 示例博客");
    assert!(
        item.note
            .as_deref()
            .is_some_and(|note| note.contains("演示"))
    );
    assert_eq!(item.created_label().len(), 10, "推荐日期是 YYYY-MM-DD");

    assert!(
        services
            .recommendation_for(DEMO_ARTICLE_NORMAL)
            .unwrap()
            .is_some()
    );
    assert!(
        services
            .recommendation_for(DEMO_ARTICLE_READ)
            .unwrap()
            .is_none()
    );

    let data: &dyn ShellData = &services;
    assert_eq!(data.recommendations().unwrap().len(), 1);
    assert!(data.recommendation(DEMO_ARTICLE_NORMAL).unwrap().is_some());
}

#[test]
fn demo_mode_refuses_recommend_writes_and_keeps_local_rows() {
    let services = demo_services();
    let error = services
        .recommend(DEMO_ARTICLE_NORMAL, Some("临时推荐".into()))
        .expect_err("demo 没有登录态，不能写推荐");
    assert!(matches!(error, TelaError::NotLoggedIn { .. }), "{error:?}");
    assert_eq!(
        services.list_recommendations().unwrap().len(),
        1,
        "写失败不应改本地推荐行"
    );
    assert_eq!(services.pending_count().unwrap(), 0, "写失败不应入队");
}
