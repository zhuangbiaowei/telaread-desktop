//! S3 阅读面板行为测试：异步加载、三视图、翻页、失败重试。
//!
//! 用真实 GPUI 窗口 + mock 内容后端；不检查像素，只断言 ReaderSession 的状态
//! 与视图方法的结果。这样“加载/重试错了”和“布局错了”能分开定位。

use std::sync::Arc;

use gpui::{AppContext, TestAppContext};
use tela_content::api::{ChapterRequest, ContentApi};
use tela_content::mock::{MOCK_CONTENT_KEY, MOCK_LONG_CONTENT_KEY, MockContentApi};
use tela_proto::TelaError;
use tela_ui::gui::ReaderView;
use tela_ui::reader::ViewMode;
use tela_ui::shell::ShellData;

struct FakeData {
    content_key: String,
}

impl ShellData for FakeData {
    fn feeds(&self) -> Result<Vec<tela_ui::shell::FeedItem>, TelaError> {
        Ok(Vec::new())
    }

    fn articles(&self, _feed_id: i64) -> Result<Vec<tela_ui::shell::ArticleItem>, TelaError> {
        Ok(Vec::new())
    }

    fn chapter_request(&self, _article_id: i64) -> Result<ChapterRequest, TelaError> {
        Ok(ChapterRequest::new(
            self.content_key.clone(),
            "zh-Hans".to_string(),
        ))
    }
}

fn data(content_key: &str) -> Arc<dyn ShellData> {
    Arc::new(FakeData {
        content_key: content_key.to_string(),
    })
}

#[gpui::test]
fn loads_chapter_in_background_and_switches_views(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let content: Arc<dyn ContentApi> = Arc::new(MockContentApi::new());
    let reader = cx.new(|cx| ReaderView::new(data(MOCK_CONTENT_KEY), content, cx));

    assert!(reader.read_with(cx, |reader, _| reader.session().state().is_loading()));
    reader.update(cx, |reader, cx| {
        reader.select(42, "Hello Tela".to_string(), cx);
    });
    // 后台取数完成、结果回到实体。
    cx.run_until_parked();

    reader.read_with(cx, |reader, _| {
        let state = reader.session();
        assert!(state.state().is_ready());
        assert_eq!(state.total_paragraphs(), 5);
        assert_eq!(state.mode(), ViewMode::Compare);
        assert_eq!(state.page(), 0);
        assert_eq!(state.page_count(), 1);
    });
}

#[gpui::test]
fn view_mode_keys_and_paging_keep_anchor(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let content: Arc<dyn ContentApi> = Arc::new(MockContentApi::new());
    let reader = cx.new(|cx| ReaderView::new(data(MOCK_LONG_CONTENT_KEY), content, cx));
    reader.update(cx, |reader, cx| {
        reader.select(43, "长章节".to_string(), cx);
    });
    cx.run_until_parked();

    assert_eq!(
        reader.read_with(cx, |reader, _| reader.session().total_paragraphs()),
        120
    );
    assert_eq!(
        reader.read_with(cx, |reader, _| reader.session().page_count()),
        6
    );

    // 翻到第 2 页，然后切三种视图：页码/锚点不能变。
    assert!(reader.update(cx, |reader, cx| reader.handle_key("n", cx)));
    assert_eq!(reader.read_with(cx, |reader, _| reader.session().page()), 1);
    let anchor = reader.read_with(cx, |reader, _| reader.session().anchor());

    assert!(reader.update(cx, |reader, cx| reader.handle_key("t", cx)));
    assert_eq!(
        reader.read_with(cx, |reader, _| reader.session().mode()),
        ViewMode::Translation
    );
    assert!(reader.update(cx, |reader, cx| reader.handle_key("o", cx)));
    assert_eq!(
        reader.read_with(cx, |reader, _| reader.session().mode()),
        ViewMode::Original
    );
    assert!(reader.update(cx, |reader, cx| reader.handle_key("c", cx)));
    assert_eq!(
        reader.read_with(cx, |reader, _| reader.session().mode()),
        ViewMode::Compare
    );
    assert_eq!(
        reader.read_with(cx, |reader, _| reader.session().anchor()),
        anchor
    );

    // 末页不能再下一页；首页不能再上一页。
    for _ in 0..4 {
        assert!(reader.update(cx, |reader, cx| reader.handle_key("n", cx)));
    }
    assert_eq!(reader.read_with(cx, |reader, _| reader.session().page()), 5);
    assert!(!reader.update(cx, |reader, cx| reader.handle_key("n", cx)));
    for _ in 0..5 {
        assert!(reader.update(cx, |reader, cx| reader.handle_key("p", cx)));
    }
    assert_eq!(reader.read_with(cx, |reader, _| reader.session().page()), 0);
    assert!(!reader.update(cx, |reader, cx| reader.handle_key("p", cx)));
}

#[gpui::test]
fn failed_load_is_retryable(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    // 第一次请求注入失败，重试时成功。
    let content: Arc<dyn ContentApi> = Arc::new(MockContentApi::with_failures(1));
    let reader = cx.new(|cx| ReaderView::new(data(MOCK_CONTENT_KEY), content, cx));
    reader.update(cx, |reader, cx| {
        reader.select(42, "会失败一次".to_string(), cx);
    });
    cx.run_until_parked();
    assert!(reader.read_with(cx, |reader, _| reader.session().state().is_failed()));

    reader.update(cx, |reader, cx| reader.reload(cx));
    assert!(reader.read_with(cx, |reader, _| reader.session().state().is_loading()));
    cx.run_until_parked();
    assert!(reader.read_with(cx, |reader, _| reader.session().state().is_ready()));
    assert_eq!(
        reader.read_with(cx, |reader, _| reader.session().retries()),
        1
    );
}

#[gpui::test]
fn missing_chapter_request_becomes_an_error_page(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    struct MissingData;
    impl ShellData for MissingData {
        fn feeds(&self) -> Result<Vec<tela_ui::shell::FeedItem>, TelaError> {
            Ok(Vec::new())
        }
        fn articles(&self, _feed_id: i64) -> Result<Vec<tela_ui::shell::ArticleItem>, TelaError> {
            Ok(Vec::new())
        }
        fn chapter_request(&self, article_id: i64) -> Result<ChapterRequest, TelaError> {
            Err(TelaError::Content {
                article: article_id,
                content_key: String::new(),
                detail: "本地没有正文对象".to_string(),
            })
        }
    }

    let content: Arc<dyn ContentApi> = Arc::new(MockContentApi::new());
    let reader = cx.new(|cx| ReaderView::new(Arc::new(MissingData), content, cx));
    reader.update(cx, |reader, cx| {
        reader.select(99, "没有正文".to_string(), cx);
    });

    // 不需要等后台任务：请求在本地装配阶段就失败了。
    assert!(reader.read_with(cx, |reader, _| reader.session().state().is_failed()));
    let message = reader.read_with(cx, |reader, _| {
        reader
            .session()
            .state()
            .error_message()
            .unwrap()
            .to_string()
    });
    assert!(message.contains("99"));
}
