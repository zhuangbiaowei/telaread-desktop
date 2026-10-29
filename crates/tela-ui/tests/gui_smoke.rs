//! S0 GUI 冒烟：用 GPUI 的真实测试 harness 打开窗口、渲染一帧、断言视图状态。
//!
//! 为什么不用 `gpui-kit/test-support`：gpui-kit 0.8.0 的 `test-support` 会引用
//! 可选的 `gpui-fast?/test-support`，Cargo 解析时会把 gpui-fast 拉进依赖图，
//! 而它与 gpui-pre 0.3.9 的 cocoa / unicode-properties 精确版本互斥（实测解析失败）。
//! GPUI 自带的 test-support 足以提供 `#[gpui::test]` 与 `TestAppContext`；
//! 元素级视觉断言由 S2/S3 的纯视图模型测试 + 真实窗口截图承担。
//!
//! 运行：`cargo test -p tela-ui --features gui-test --test gui_smoke`

use gpui::{
    AppContext, Bounds, Point, Size, TestAppContext, WindowBounds, WindowOptions, px, size,
};

fn open_window<V: gpui::Render>(
    cx: &mut TestAppContext,
    size: Option<Size<gpui::Pixels>>,
    build: impl FnOnce(&mut gpui::Window, &mut gpui::App) -> gpui::Entity<V>,
) -> (gpui::AnyWindowHandle, gpui::Entity<V>) {
    cx.update(|cx| {
        let bounds = size
            .map(|size| Bounds {
                origin: Point::default(),
                size,
            })
            .unwrap_or_else(|| Bounds::maximized(None, cx));
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            build,
        )
        .expect("open test window")
    })
}

#[gpui::test]
fn boots_and_renders_a_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);

    let (window, view) = open_window(cx, Some(size(px(1100.), px(720.))), |_, cx| {
        cx.new(|_| tela_ui::gui::BootView::new())
    });

    cx.update_window(window, |_, window, cx| {
        // 完成一帧：窗口、元素树与绘制资源都必须真实初始化。
        window.draw(cx).clear(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.title().to_string()),
            tela_ui::gui::WINDOW_TITLE
        );
        assert_eq!(cx.windows().len(), 1);
    })
    .unwrap();
}
