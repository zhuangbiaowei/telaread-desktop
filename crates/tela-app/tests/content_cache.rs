//! 本地优先加载与订阅级离线缓存：第二次打开不再联网，按钮能批量缓存原文/译文。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tela_app::services::{
    AppConfig, AppServices, DEMO_ARTICLE_NORMAL, DEMO_FEED_ID, DEMO_FEED_ID_2,
};
use tela_content::api::{ChapterRequest, ContentApi};
use tela_content::mock::MockContentApi;
use tela_content::model::{
    Chapter, ContentBlock, ContentObject, ContentStats, TranslationAnswer, TranslationObject,
    TranslationRow, TranslationState, TranslationView,
};
use tela_net::MockTransport;
use tela_proto::TelaError;

/// 记录远端调用的 mock：用来证明“第二次打开没有再发请求”。
struct CountingApi {
    inner: MockContentApi,
    loads: AtomicUsize,
    translation_requests: AtomicUsize,
    images: AtomicUsize,
}

impl CountingApi {
    fn new() -> Self {
        Self {
            inner: MockContentApi::new(),
            loads: AtomicUsize::new(0),
            translation_requests: AtomicUsize::new(0),
            images: AtomicUsize::new(0),
        }
    }

    fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }

    fn translation_requests(&self) -> usize {
        self.translation_requests.load(Ordering::SeqCst)
    }

    fn images(&self) -> usize {
        self.images.load(Ordering::SeqCst)
    }
}

impl ContentApi for CountingApi {
    fn load_chapter(&self, request: &ChapterRequest) -> Result<Chapter, TelaError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.inner.load_chapter(request)
    }

    fn request_translation(
        &self,
        article_id: i64,
        lang: &str,
    ) -> Result<TranslationAnswer, TelaError> {
        self.translation_requests.fetch_add(1, Ordering::SeqCst);
        self.inner.request_translation(article_id, lang)
    }

    fn translation_status(
        &self,
        content_key: &str,
        lang: &str,
    ) -> Result<Option<TranslationRow>, TelaError> {
        self.inner.translation_status(content_key, lang)
    }

    fn load_translation_object(&self, path: &str) -> Result<TranslationObject, TelaError> {
        self.inner.load_translation_object(path)
    }

    fn load_image(&self, src: &str) -> Result<Vec<u8>, TelaError> {
        self.images.fetch_add(1, Ordering::SeqCst);
        self.inner.load_image(src)
    }
}

fn services(api: Arc<CountingApi>) -> AppServices {
    let services = AppServices::open_in_memory(
        "zh-Hans",
        api as Arc<dyn ContentApi>,
        Arc::new(MockTransport::new()),
    )
    .expect("open in-memory services");
    services.seed_demo().expect("seed demo");
    services
}

#[test]
fn second_open_reads_body_and_translation_from_local_cache() {
    let api = Arc::new(CountingApi::new());
    let services = services(Arc::clone(&api));

    let first = services.load_chapter(DEMO_ARTICLE_NORMAL).unwrap();
    assert_eq!(first.total_blocks(), 5);
    assert!(
        matches!(first.translation, TranslationView::Ready { .. }),
        "第一次打开应拿到完整译文"
    );
    assert_eq!(api.loads(), 1);
    let status = services.sync_status().unwrap();
    assert_eq!(status.bodies, 1, "正文应写入本地库");
    assert_eq!(status.translations, 1, "译文也应写入本地库");

    // 第二次打开：正文与译文都从本地读，不再联网。
    let second = services.load_chapter(DEMO_ARTICLE_NORMAL).unwrap();
    assert_eq!(second.total_blocks(), 5);
    assert!(matches!(second.translation, TranslationView::Ready { .. }));
    assert_eq!(api.loads(), 1, "第二次打开不应再调用远端");
    assert_eq!(api.translation_requests(), 0, "已缓存译文不应重复请求翻译");

    // 模拟断网后仍然可读（完全走本地缓存）。
    api.inner.fail_next(10);
    let offline = services.load_chapter(DEMO_ARTICLE_NORMAL).unwrap();
    assert_eq!(offline.total_blocks(), 5);
    assert!(matches!(offline.translation, TranslationView::Ready { .. }));
    assert_eq!(api.loads(), 1);
}

#[test]
fn body_and_translation_cache_survive_reopening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tela.db");
    let api = Arc::new(CountingApi::new());
    {
        let services = AppServices::open(
            AppConfig::for_tests(&path),
            Arc::clone(&api) as Arc<dyn ContentApi>,
            Arc::new(MockTransport::new()),
        )
        .unwrap();
        services.seed_demo().unwrap();
        services.load_chapter(DEMO_ARTICLE_NORMAL).unwrap();
    }
    let loads = api.loads();
    {
        let services = AppServices::open(
            AppConfig::for_tests(&path),
            Arc::clone(&api) as Arc<dyn ContentApi>,
            Arc::new(MockTransport::new()),
        )
        .unwrap();
        // 重开进程后也应只读本地库。
        let chapter = services.load_chapter(DEMO_ARTICLE_NORMAL).unwrap();
        assert_eq!(chapter.total_blocks(), 5);
        assert!(matches!(chapter.translation, TranslationView::Ready { .. }));
        assert_eq!(api.loads(), loads, "重开库后不应重新下载正文/译文");
    }
}

#[test]
fn cache_feed_prefetches_all_uncached_originals_and_translations() {
    let api = Arc::new(CountingApi::new());
    let services = services(Arc::clone(&api));
    let cache = services.offline_cache();

    let progress = std::sync::Mutex::new(Vec::new());
    let report = cache
        .cache_feed(DEMO_FEED_ID, &|snapshot| {
            if let Ok(mut progress) = progress.lock() {
                progress.push(snapshot);
            }
        })
        .expect("cache feed");
    let progress = progress.into_inner().unwrap();

    // 演示订阅 1 有 3 篇文章（正常 / 长章节 / 无译文）。
    assert_eq!(report.total, 3);
    assert_eq!(report.originals_cached, 3, "3 篇原文都应在本次缓存");
    assert_eq!(report.translations_cached, 1, "只有正常章节有译文 fixture");
    assert_eq!(report.images_cached, 1, "正常章节的封面图也应缓存");
    assert_eq!(report.failed, 0);
    assert_eq!(report.pending, 0);

    let status = services.sync_status().unwrap();
    assert_eq!(status.bodies, 3);
    assert_eq!(status.translations, 3, "含无译文快照");
    assert_eq!(status.images, 1);

    let last = progress.last().expect("至少回调一次进度");
    assert_eq!(last.total, 3);
    assert_eq!(last.done, 3);
    assert!(!last.finished, "finished 由 UI 侧在任务结束后补上");
    assert!(
        progress.iter().any(|snapshot| snapshot.done > 0),
        "进度应逐篇推进"
    );

    // 再点一次：已完整缓存的文章和图片都跳过，不重复下载。
    let loads_before = api.loads();
    let images_before = api.images();
    let report = cache.cache_feed(DEMO_FEED_ID, &|_| {}).expect("cache feed");
    assert_eq!(report.total, 3);
    assert_eq!(report.originals_cached, 0);
    assert_eq!(report.images_cached, 0);
    assert!(report.skipped >= 1, "完整缓存的文章应被跳过");
    assert_eq!(api.loads(), loads_before, "已缓存原文不应重新下载");
    assert_eq!(api.images(), images_before, "已缓存图片不应重新下载");
}

#[test]
fn image_bytes_are_cached_and_reused_without_network() {
    let api = Arc::new(CountingApi::new());
    let services = services(Arc::clone(&api));
    let src = "https://cdn.example.com/cover.png";

    let first = services.content().load_image(src).unwrap();
    assert!(!first.is_empty());
    assert_eq!(api.images(), 1);

    let second = services.content().load_image(src).unwrap();
    assert_eq!(second, first, "第二次应返回本地缓存的同一份字节");
    assert_eq!(api.images(), 1, "第二次读图不应再调用远端");
    assert_eq!(services.sync_status().unwrap().images, 1);
}

/// 第一次打开时译文还在生成：本地缓存 pending，之后轮询必须绕过缓存拿到 ready。
struct PendingThenReadyApi {
    loads: AtomicUsize,
    views: AtomicUsize,
}

impl PendingThenReadyApi {
    fn new() -> Self {
        Self {
            loads: AtomicUsize::new(0),
            views: AtomicUsize::new(0),
        }
    }
}

impl ContentApi for PendingThenReadyApi {
    fn load_chapter(&self, request: &ChapterRequest) -> Result<Chapter, TelaError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(Chapter::new(
            ContentObject {
                format: 1,
                norm: 0,
                key: request.content_key.clone(),
                lang: "en".to_string(),
                blocks: vec![ContentBlock {
                    tag: "p".to_string(),
                    html: "<p>Hello</p>".to_string(),
                    leaves: Vec::new(),
                }],
                leaves: Default::default(),
                images: Vec::new(),
                stats: ContentStats::default(),
            },
            TranslationView::Pending {
                state: TranslationState::Running,
                due_at: None,
            },
        ))
    }

    fn translation_view(
        &self,
        content: &ContentObject,
        lang: &str,
    ) -> Result<TranslationView, TelaError> {
        self.views.fetch_add(1, Ordering::SeqCst);
        Ok(TranslationView::from_object(
            content.total_blocks(),
            TranslationObject {
                format: 1,
                norm: 0,
                source: content.key.clone(),
                lang: lang.to_string(),
                status: "done".to_string(),
                blocks: vec!["你好".to_string()],
                failed_leaves: Vec::new(),
            },
        ))
    }
}

#[test]
fn polling_translation_bypasses_cache_and_lands_in_local_storage() {
    let api = Arc::new(PendingThenReadyApi::new());
    let services = AppServices::open_in_memory(
        "zh-Hans",
        Arc::clone(&api) as Arc<dyn ContentApi>,
        Arc::new(MockTransport::new()),
    )
    .unwrap();
    let request = ChapterRequest::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "zh-Hans");

    let first = services.content().load_chapter(&request).unwrap();
    assert!(matches!(first.translation, TranslationView::Pending { .. }));
    assert_eq!(api.loads.load(Ordering::SeqCst), 1);

    // 第二次打开：正文与 pending 快照都来自本地。
    let second = services.content().load_chapter(&request).unwrap();
    assert!(matches!(
        second.translation,
        TranslationView::Pending { .. }
    ));
    assert_eq!(api.loads.load(Ordering::SeqCst), 1);
    assert_eq!(api.views.load(Ordering::SeqCst), 0, "打开不应刷新译文");

    // 阅读面板轮询：只重新查译文，正文不重复下载。
    let refreshed = services.content().refresh_chapter(&request).unwrap();
    assert!(matches!(
        refreshed.translation,
        TranslationView::Ready { .. }
    ));
    assert_eq!(api.loads.load(Ordering::SeqCst), 1);
    assert_eq!(api.views.load(Ordering::SeqCst), 1);

    // ready 已写回本地，下一次打开完全离线。
    let third = services.content().load_chapter(&request).unwrap();
    assert!(matches!(third.translation, TranslationView::Ready { .. }));
    assert_eq!(api.loads.load(Ordering::SeqCst), 1);
    assert_eq!(api.views.load(Ordering::SeqCst), 1);
    assert_eq!(services.sync_status().unwrap().translations, 1);
}

/// 可控网络状态的 API：断网时所有远端调用返回 `Net`，探活也失败。
struct OfflineAwareApi {
    online: std::sync::atomic::AtomicBool,
    loads: AtomicUsize,
    views: AtomicUsize,
    probes: AtomicUsize,
}

impl OfflineAwareApi {
    fn new() -> Self {
        Self {
            online: std::sync::atomic::AtomicBool::new(true),
            loads: AtomicUsize::new(0),
            views: AtomicUsize::new(0),
            probes: AtomicUsize::new(0),
        }
    }

    fn set_online(&self, online: bool) {
        self.online.store(online, Ordering::SeqCst);
    }

    fn offline() -> TelaError {
        TelaError::Net {
            ctx: tela_proto::WireContext::new("mock://offline", "probe"),
            detail: "网络不可用".into(),
        }
    }
}

impl ContentApi for OfflineAwareApi {
    fn load_chapter(&self, request: &ChapterRequest) -> Result<Chapter, TelaError> {
        if !self.online.load(Ordering::SeqCst) {
            return Err(Self::offline());
        }
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(Chapter::new(
            ContentObject {
                format: 1,
                norm: 0,
                key: request.content_key.clone(),
                lang: "en".to_string(),
                blocks: vec![ContentBlock {
                    tag: "p".to_string(),
                    html: "<p>Hello</p>".to_string(),
                    leaves: Vec::new(),
                }],
                leaves: Default::default(),
                images: Vec::new(),
                stats: ContentStats::default(),
            },
            TranslationView::Pending {
                state: TranslationState::Running,
                due_at: None,
            },
        ))
    }

    fn translation_view(
        &self,
        content: &ContentObject,
        lang: &str,
    ) -> Result<TranslationView, TelaError> {
        self.views.fetch_add(1, Ordering::SeqCst);
        if !self.online.load(Ordering::SeqCst) {
            return Err(Self::offline());
        }
        Ok(TranslationView::from_object(
            content.total_blocks(),
            TranslationObject {
                format: 1,
                norm: 0,
                source: content.key.clone(),
                lang: lang.to_string(),
                status: "done".to_string(),
                blocks: vec!["你好".to_string()],
                failed_leaves: Vec::new(),
            },
        ))
    }

    fn probe_network(&self) -> Result<(), TelaError> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        if self.online.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(Self::offline())
        }
    }
}

#[test]
fn offline_pauses_translation_requests_until_probe_succeeds() {
    let api = Arc::new(OfflineAwareApi::new());
    let services = AppServices::open_in_memory(
        "zh-Hans",
        Arc::clone(&api) as Arc<dyn ContentApi>,
        Arc::new(MockTransport::new()),
    )
    .unwrap();
    let request = ChapterRequest::new("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "zh-Hans");
    let content = services.content();

    // 第一次打开：正文 + pending 快照入本地。
    assert!(matches!(
        content.load_chapter(&request).unwrap().translation,
        TranslationView::Pending { .. }
    ));
    assert_eq!(api.loads.load(Ordering::SeqCst), 1);

    api.set_online(false);
    // 第一次刷新发现断网 → 标记离线。
    assert!(content.refresh_chapter(&request).is_err());
    assert!(content.is_offline());
    assert_eq!(api.views.load(Ordering::SeqCst), 1);

    // 离线后：打开文章只读本地，不发任何译文状态请求。
    let local = content.load_chapter(&request).unwrap();
    assert!(matches!(local.translation, TranslationView::Pending { .. }));
    assert_eq!(api.loads.load(Ordering::SeqCst), 1);
    assert_eq!(api.views.load(Ordering::SeqCst), 1);

    // 再次刷新只会做一次轻量探活，不碰译文接口。
    assert!(content.refresh_chapter(&request).is_err());
    assert_eq!(api.views.load(Ordering::SeqCst), 1, "离线时不应请求译文");
    assert_eq!(api.probes.load(Ordering::SeqCst), 1, "只允许探活一次");

    // 网络恢复后探活成功，状态回到在线，轮询可以继续。
    api.set_online(true);
    content.probe_network().expect("网络已恢复");
    assert!(!content.is_offline());
    let refreshed = content.refresh_chapter(&request).unwrap();
    assert!(matches!(
        refreshed.translation,
        TranslationView::Ready { .. }
    ));
    assert_eq!(api.views.load(Ordering::SeqCst), 2);
    assert_eq!(api.probes.load(Ordering::SeqCst), 2);

    // 成品已回写本地；再打开只读本地。
    let cached = content.load_chapter(&request).unwrap();
    assert!(matches!(cached.translation, TranslationView::Ready { .. }));
    assert_eq!(api.loads.load(Ordering::SeqCst), 1);
    assert_eq!(api.views.load(Ordering::SeqCst), 2);
}

#[test]
fn cache_feed_on_a_feed_without_articles_is_a_no_op() {
    let api = Arc::new(CountingApi::new());
    let services = services(api);
    // 订阅 2 有 1 篇文章；先缓存一次，再确认空订阅也能正常返回。
    let cache = services.offline_cache();
    let report = cache.cache_feed(DEMO_FEED_ID_2, &|_| {}).unwrap();
    assert_eq!(report.total, 1);
    let report = cache.cache_feed(9_999, &|_| {}).unwrap();
    assert_eq!(report.total, 0);
    assert_eq!(report.originals_cached, 0);
    assert_eq!(report.skipped, 0);
}
