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

// ---------------------------------------------------------------------------
// 网页收藏（Shift+B）
// ---------------------------------------------------------------------------

const BOOKMARK_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <title>Saved Page</title>
    <meta property="og:site_name" content="Example Blog">
  </head>
  <body>
    <article>
      <h1>Saved Page</h1>
      <p>A paragraph that is long enough to be extracted as the article body,
         so the reader has something real to show after bookmarking.</p>
      <p>Second paragraph.</p>
    </article>
  </body>
</html>"#;

/// 用内联 HTML 走一遍「解析 → 落库」，不联网。
fn save_test_bookmark(services: &AppServices, url: &str) -> tela_ui::shell::BookmarkItem {
    let page = tela_content::webpage::parse_html(url, BOOKMARK_HTML);
    services.save_bookmark_page(&page).expect("save bookmark")
}

#[test]
fn bookmark_is_saved_locally_with_title_body_and_request() {
    let services =
        AppServices::open_in_memory("zh-Hans", content(), Arc::new(MockTransport::new())).unwrap();
    assert!(services.list_bookmarks().unwrap().is_empty());

    let item = save_test_bookmark(&services, "https://example.com/saved-page");
    assert_eq!(item.title, "Saved Page");
    assert_eq!(item.site, "Example Blog");
    assert_eq!(item.url, "https://example.com/saved-page");
    assert_eq!(item.image_count, 0);
    assert_eq!(item.created_label().len(), 10);

    let list = services.list_bookmarks().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, item.id);

    // 正文请求指向本地 `web-` key，阅读面板据此读本地库。
    let request = services.bookmark_request_for(item.id).unwrap();
    assert_eq!(
        request.content_key,
        tela_content::webpage::content_key_for_url("https://example.com/saved-page")
    );
    assert!(tela_content::webpage::is_local_page_key(
        &request.content_key
    ));
    // Markdown 存进本地库后，阅读面板能渲染成块。
    let chapter = services
        .content()
        .load_chapter(&request)
        .expect("读收藏正文");
    assert!(!chapter.content.blocks.is_empty(), "正文应渲染出块");
    assert!(
        matches!(chapter.translation, TranslationView::NotAvailable),
        "收藏网页没有译文"
    );

    // 通过 ShellData 暴露给 GUI。
    let data: &dyn ShellData = &services;
    assert_eq!(data.bookmarks().unwrap().len(), 1);
    assert_eq!(
        data.bookmark_request(item.id).unwrap().content_key,
        request.content_key
    );
}

#[test]
fn bookmarking_the_same_url_twice_refreshes_one_row() {
    let services =
        AppServices::open_in_memory("zh-Hans", content(), Arc::new(MockTransport::new())).unwrap();
    let first = save_test_bookmark(&services, "https://example.com/saved-page");
    let second = save_test_bookmark(&services, "https://example.com/saved-page");

    assert_eq!(first.id, second.id, "同一个 URL 只有一条收藏");
    assert_eq!(services.list_bookmarks().unwrap().len(), 1);
}

#[test]
fn removing_a_bookmark_clears_its_markdown() {
    let services =
        AppServices::open_in_memory("zh-Hans", content(), Arc::new(MockTransport::new())).unwrap();
    let item = save_test_bookmark(&services, "https://example.com/saved-page");
    let request = services.bookmark_request_for(item.id).unwrap();
    assert!(
        services.content().load_chapter(&request).is_ok(),
        "收藏的 Markdown 应能读出来"
    );

    services.remove_bookmark(item.id).unwrap();
    assert!(services.list_bookmarks().unwrap().is_empty());
    assert!(
        services.bookmark_request_for(item.id).is_err(),
        "删除后不能再取正文请求"
    );
    assert!(
        services.content().load_chapter(&request).is_err(),
        "Markdown 正文应随收藏一起清理"
    );
    // 再次删除是幂等的。
    services.remove_bookmark(item.id).unwrap();
}

#[test]
fn bookmark_markdown_is_the_canonical_stored_format() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tela.db");
    let services = AppServices::open(
        AppConfig::for_tests(&path),
        content(),
        Arc::new(MockTransport::new()),
    )
    .unwrap();
    let item = save_test_bookmark(&services, "https://example.com/saved-page");

    // 关库再打开：Markdown 在本地库里，图片地址保持原始绝对地址。
    drop(services);
    let services = AppServices::open(
        AppConfig::for_tests(&path),
        content(),
        Arc::new(MockTransport::new()),
    )
    .unwrap();
    let request = services.bookmark_request_for(item.id).unwrap();
    let chapter = services
        .content()
        .load_chapter(&request)
        .expect("重开库后读正文");
    let html: String = chapter
        .content
        .blocks
        .iter()
        .map(|block| block.html.as_str())
        .collect();
    assert!(
        html.contains("Saved Page"),
        "Markdown 标题应渲染出来：{html}"
    );
    assert!(html.contains("Second paragraph"), "正文应渲染出来：{html}");
}

#[test]
fn bookmark_requires_a_valid_url_before_any_network_call() {
    let services =
        AppServices::open_in_memory("zh-Hans", content(), Arc::new(MockTransport::new())).unwrap();
    let error = services
        .add_bookmark("javascript:alert(1)")
        .expect_err("非 http(s) 地址必须被拒绝");
    assert!(matches!(error, TelaError::Config { .. }), "{error:?}");
    assert!(services.list_bookmarks().unwrap().is_empty());

    let error = services.add_bookmark("   ").expect_err("空地址必须被拒绝");
    assert!(matches!(error, TelaError::Config { .. }), "{error:?}");
}

/// 真实网络冒烟：`add_bookmark` 抓取 → 抽取 → 落库 → 阅读。
///
/// ```bash
/// cargo test -p tela-app --test services_v2 -- --ignored
/// ```
#[test]
#[ignore = "需要网络：手动运行验证真实收藏"]
fn add_bookmark_downloads_a_real_page_and_reads_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let services = AppServices::open(
        AppConfig::for_tests(dir.path().join("tela.db")),
        content(),
        Arc::new(MockTransport::new()),
    )
    .unwrap();

    let item = services
        .add_bookmark("example.com")
        .expect("抓取并收藏 example.com");
    assert_eq!(item.url, "https://example.com/");
    assert_eq!(services.list_bookmarks().unwrap().len(), 1);

    let request = services.bookmark_request_for(item.id).unwrap();
    let chapter = services
        .content()
        .load_chapter(&request)
        .expect("读收藏正文");
    assert!(!chapter.is_empty());
    assert_eq!(chapter.content.lang, "en");
    assert!(matches!(chapter.translation, TranslationView::NotAvailable));
}

/// 微信文章端到端：抓取 → 抽取 → 落库 → 阅读（用微信内 UA，不保存验证页）。
///
/// ```bash
/// cargo test -p tela-app --test services_v2 -- --ignored --nocapture
/// ```
#[test]
#[ignore = "需要网络：手动验证公众号文章收藏"]
fn add_bookmark_fetches_a_wechat_article() {
    let dir = tempfile::tempdir().unwrap();
    let services = AppServices::open(
        AppConfig::for_tests(dir.path().join("tela.db")),
        content(),
        Arc::new(MockTransport::new()),
    )
    .unwrap();

    let item = services
        .add_bookmark("https://mp.weixin.qq.com/s/F0PJHTfbn-Jt1kvHmWZ-vg?scene=1")
        .expect("收藏公众号文章");
    assert!(
        item.title.contains("大模型"),
        "标题应来自文章（不是“环境异常”）：{item:?}"
    );
    assert_eq!(item.site, "微信公众平台");

    let request = services.bookmark_request_for(item.id).unwrap();
    let chapter = services
        .content()
        .load_chapter(&request)
        .expect("读收藏正文");
    assert!(
        chapter.content.blocks.len() > 10,
        "公众号文章应有多个正文块，实际 {}",
        chapter.content.blocks.len()
    );
    eprintln!(
        "wechat bookmark: title={:?} blocks={} chars={}",
        item.title,
        chapter.content.blocks.len(),
        chapter.content.stats.chars
    );
}

/// X（Twitter）帖子端到端：标题取正文第一行、正文是长推文全文。
///
/// ```bash
/// cargo test -p tela-app --test services_v2 -- --ignored --nocapture
/// ```
#[test]
#[ignore = "需要网络：手动验证 X 帖子收藏"]
fn add_bookmark_fetches_an_x_post_with_a_smart_title() {
    let dir = tempfile::tempdir().unwrap();
    let services = AppServices::open(
        AppConfig::for_tests(dir.path().join("tela.db")),
        content(),
        Arc::new(MockTransport::new()),
    )
    .unwrap();

    let item = services
        .add_bookmark("https://x.com/UPing123zzz/status/2116729183334523014")
        .expect("收藏 X 帖子");
    eprintln!("x bookmark: {item:?}");
    assert_eq!(item.site, "X");
    assert!(
        item.title.starts_with("我现在越来越觉得"),
        "标题应是正文第一行：{item:?}"
    );

    let request = services.bookmark_request_for(item.id).unwrap();
    let chapter = services
        .content()
        .load_chapter(&request)
        .expect("读收藏正文");
    assert!(chapter.content.blocks.len() > 10);
    assert!(
        chapter.content.stats.chars > 1_000,
        "长推文应保存全文，实际 {} 字",
        chapter.content.stats.chars
    );
}

/// X 长文章端到端：收藏 `/user/article/<id>` 后标题、正文块与图片都应落库。
///
/// ```bash
/// cargo test -p tela-app --test services_v2 -- --ignored --nocapture
/// ```
#[test]
#[ignore = "需要网络：手动验证 X 文章收藏"]
fn add_bookmark_fetches_an_x_article() {
    let dir = tempfile::tempdir().unwrap();
    let services = AppServices::open(
        AppConfig::for_tests(dir.path().join("tela.db")),
        content(),
        Arc::new(MockTransport::new()),
    )
    .unwrap();

    let item = services
        .add_bookmark("https://x.com/SuJinyan6/article/2116730566678868190")
        .expect("收藏 X 文章");
    eprintln!("x article bookmark: {item:?}");
    assert_eq!(item.site, "X");
    assert_eq!(item.title, "2026 北美普通 PhD 找工小结");
    assert!(item.image_count > 0, "文章图片应下载缓存：{item:?}");

    let request = services.bookmark_request_for(item.id).unwrap();
    let chapter = services
        .content()
        .load_chapter(&request)
        .expect("读收藏正文");
    assert!(chapter.content.blocks.len() > 40);
    assert!(
        chapter.content.stats.chars > 5_000,
        "文章应保存全文，实际 {} 字",
        chapter.content.stats.chars
    );
    assert!(!chapter.content.images.is_empty());
}

/// 图片链路：Markdown 里保留绝对图片地址，图片字节进本地缓存，离线也能读。
#[test]
#[ignore = "需要网络：手动验证带图片的网页收藏"]
fn add_bookmark_caches_images_from_markdown() {
    let dir = tempfile::tempdir().unwrap();
    let services = AppServices::open(
        AppConfig::for_tests(dir.path().join("tela.db")),
        content(),
        Arc::new(MockTransport::new()),
    )
    .unwrap();

    let item = services
        .add_bookmark("https://en.wikipedia.org/wiki/Rust_(programming_language)")
        .expect("收藏 Wikipedia");
    eprintln!(
        "wiki bookmark: title={:?} images={} ",
        item.title, item.image_count
    );
    assert!(item.image_count > 0, "页面图片应已下载：{item:?}");

    let request = services.bookmark_request_for(item.id).unwrap();
    let chapter = services
        .content()
        .load_chapter(&request)
        .expect("读收藏正文");
    assert!(!chapter.content.images.is_empty(), "正文应包含图片");
    let src = chapter.content.images[0].clone();
    assert!(src.starts_with("http"), "图片地址应是绝对地址：{src}");
    let bytes = services
        .content()
        .load_image(&src)
        .expect("图片应从本地缓存读出");
    assert!(!bytes.is_empty());
}
