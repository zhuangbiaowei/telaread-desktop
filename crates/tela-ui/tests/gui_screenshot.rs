//! 本地截图工具（默认忽略）：用 GPUI 的 WGPU headless renderer 把三栏 + 阅读面板
//! 渲染成 PNG，供 README/验收文档使用。CI 没有 GPU 适配器，因此不进自动化门禁。
//!
//! 运行：
//! ```bash
//! cargo test -p tela-ui --features gui-test --test gui_screenshot -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AppContext, HeadlessAppContext, px, size};
use tela_content::api::{ChapterRequest, ContentApi};
use tela_content::mock::{MOCK_CONTENT_KEY, MockContentApi};
use tela_proto::TelaError;
use tela_ui::gui::ShellView;
use tela_ui::shell::{ArticleItem, FeedItem, ShellData};

struct DemoData;

impl ShellData for DemoData {
    fn feeds(&self) -> Result<Vec<FeedItem>, TelaError> {
        Ok(vec![
            FeedItem {
                id: 1,
                title: "Tela 示例博客".to_string(),
                feed_url: "https://example.com/tela.xml".to_string(),
                unread: 3,
                article_count: 3,
                status: "active".to_string(),
            },
            FeedItem {
                id: 2,
                title: "技术随笔".to_string(),
                feed_url: "https://example.com/rust-weekly.xml".to_string(),
                unread: 0,
                article_count: 1,
                status: "active".to_string(),
            },
        ])
    }

    fn articles(&self, feed_id: i64) -> Result<Vec<ArticleItem>, TelaError> {
        let mut map = HashMap::new();
        map.insert(
            1,
            vec![
                ArticleItem {
                    id: 42,
                    feed_id: 1,
                    title: "Hello Tela：一篇文章的完整对照".to_string(),
                    unread: true,
                    published_at: Some(1_761_350_700),
                    excerpt: Some("欢迎来到 Tela Desktop。这一段原文来自离线 fixture，右侧是已经对齐的中文译文。".to_string()),
                },
                ArticleItem {
                    id: 43,
                    feed_id: 1,
                    title: "长章节：120 段分页演示".to_string(),
                    unread: true,
                    published_at: Some(1_761_350_600),
                    excerpt: Some("这一章有 120 个段落，用来看长文分页与阅读位置保持。".to_string()),
                },
                ArticleItem {
                    id: 44,
                    feed_id: 1,
                    title: "没有译文时仍然能读原文".to_string(),
                    unread: true,
                    published_at: Some(1_761_350_500),
                    excerpt: Some("这一篇没有译文对象，界面应显式提示并回退到原文。".to_string()),
                },
            ],
        );
        map.insert(
            2,
            vec![ArticleItem {
                id: 45,
                feed_id: 2,
                title: "Rust 周报 #1：已读文章".to_string(),
                unread: false,
                published_at: Some(1_761_350_400),
                excerpt: Some("这是一篇已读文章，用来验证列表里的已读/未读区分。".to_string()),
            }],
        );
        Ok(map.remove(&feed_id).unwrap_or_default())
    }

    fn chapter_request(&self, _article_id: i64) -> Result<ChapterRequest, TelaError> {
        Ok(ChapterRequest::new(MOCK_CONTENT_KEY, "zh-Hans"))
    }
}

fn content() -> Arc<dyn ContentApi> {
    Arc::new(MockContentApi::new())
}

#[test]
#[ignore = "需要本机 WGPU headless renderer；CI 无 GPU 适配器"]
fn captures_the_three_pane_reader() {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(gpui_kit::assets::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(gpui_kit::init);

    let data: Arc<dyn ShellData> = Arc::new(DemoData);
    let content = content();
    let handle = cx
        .open_window(size(px(1100.), px(720.)), move |_, cx| {
            cx.new(|cx| ShellView::new(data, content, cx))
        })
        .expect("open headless window");

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();

    // 等后台正文取数完成，再渲染一帧。
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();

    let image = cx
        .capture_screenshot(handle.into())
        .expect("capture screenshot");
    // 测试进程的 CWD 是 crate 根目录；截图统一落到 workspace 的 target/ 下（已忽略）。
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("target/gui-screenshots");
    std::fs::create_dir_all(&dir).expect("create screenshot dir");
    let path = dir.join("s3-reader.png");
    image.save(&path).expect("save screenshot");
    eprintln!("saved {}", path.display());
}
