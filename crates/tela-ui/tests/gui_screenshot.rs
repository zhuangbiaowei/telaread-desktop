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
use tela_ui::shell::{ArticleItem, BookmarkItem, BookmarkService, FeedItem, ShellData};

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
                    url: Some("https://example.com/articles/42".to_string()),
                    title: "Hello Tela：一篇文章的完整对照".to_string(),
                    unread: true,
                    published_at: Some(1_761_350_700),
                    excerpt: Some("欢迎来到 Tela Desktop。这一段原文来自离线 fixture，右侧是已经对齐的中文译文。".to_string()),
                },
                ArticleItem {
                    id: 43,
                    feed_id: 1,
                    url: Some("https://example.com/articles/43".to_string()),
                    title: "长章节：120 段连续滚动演示".to_string(),
                    unread: true,
                    published_at: Some(1_761_350_600),
                    excerpt: Some(
                        "这一章有 120 个段落，用来看长文连续滚动与阅读位置保持。".to_string(),
                    ),
                },
                ArticleItem {
                    id: 44,
                    feed_id: 1,
                    url: Some("https://example.com/articles/44".to_string()),
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
                url: Some("https://example.com/articles/45".to_string()),
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

    fn bookmarks(&self) -> Result<Vec<BookmarkItem>, TelaError> {
        Ok(vec![
            BookmarkItem {
                id: 1,
                url: "https://example.com/bookmarks/offline-reading".to_string(),
                title: "离线阅读的三种做法".to_string(),
                site: "Tela 示例博客".to_string(),
                image_count: 2,
                created_at: 1_761_350_700_000,
            },
            BookmarkItem {
                id: 2,
                url: "https://example.com/bookmarks/rust-in-2026".to_string(),
                title: "Rust 2026：值得关注的五件事".to_string(),
                site: "技术随笔".to_string(),
                image_count: 0,
                created_at: 1_761_350_400_000,
            },
        ])
    }

    fn bookmark_request(&self, _bookmark_id: i64) -> Result<ChapterRequest, TelaError> {
        Ok(ChapterRequest::new(MOCK_CONTENT_KEY, "zh-Hans"))
    }
}

/// 截图用的收藏服务替身：只提供能力，不真的联网。
struct StubBookmarks;

impl BookmarkService for StubBookmarks {
    fn add_bookmark(&self, url: &str) -> Result<BookmarkItem, TelaError> {
        Err(TelaError::Config {
            detail: format!("截图模式不下载 {url}"),
        })
    }

    fn remove_bookmark(&self, _bookmark_id: i64) -> Result<(), TelaError> {
        Ok(())
    }
}

fn screenshot_dir() -> std::path::PathBuf {
    // 测试进程的 CWD 是 crate 根目录；截图统一落到 workspace 的 target/ 下（已忽略）。
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("target/gui-screenshots")
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

/// 「我的收藏」列表 + `Shift+B` 收藏弹窗的截图（README/验收用）。
#[test]
#[ignore = "需要本机 WGPU headless renderer；CI 无 GPU 适配器"]
fn captures_bookmarks_pane_and_dialog() {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(gpui_kit::assets::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(gpui_kit::init);

    let data: Arc<dyn ShellData> = Arc::new(DemoData);
    let content = content();
    let bookmarks: Arc<dyn BookmarkService> = Arc::new(StubBookmarks);
    let handle = cx
        .open_window(size(px(1100.), px(720.)), move |_, cx| {
            cx.new(|cx| {
                ShellView::new_with_bookmarks(data, content, None, None, None, Some(bookmarks), cx)
            })
        })
        .expect("open headless window");
    let view = cx.update(|cx| handle.entity(cx)).expect("root view entity");

    // 首帧 + 后台正文取数完成后再渲染一帧。
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();

    // 切到「我的收藏」并打开第一条收藏。
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            assert!(view.show_bookmarks(cx));
            assert!(view.select_bookmark(1, cx));
            cx.notify();
        });
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();

    let dir = screenshot_dir();
    std::fs::create_dir_all(&dir).expect("create screenshot dir");
    let image = cx
        .capture_screenshot(handle.into())
        .expect("capture bookmarks screenshot");
    let path = dir.join("s8-bookmarks.png");
    image.save(&path).expect("save screenshot");
    eprintln!("saved {}", path.display());

    // Shift+B 收藏弹窗。
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| view.open_bookmark_dialog(window, cx));
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();

    let image = cx
        .capture_screenshot(handle.into())
        .expect("capture dialog screenshot");
    let path = dir.join("s8-bookmark-dialog.png");
    image.save(&path).expect("save screenshot");
    eprintln!("saved {}", path.display());
}
