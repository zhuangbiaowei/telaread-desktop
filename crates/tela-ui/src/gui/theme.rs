//! GUI 侧的主题接入：把 [`crate::theme::TelaTheme`] 放进 GPUI 全局，
//! 并把主题里的图标名解析成 gpui-kit 的 Lucide 图标。
//!
//! 视图只通过 `cx.tela_theme()` 取主题，不各自缓存一份：切换主题时外壳与阅读
//! 面板看到的是同一份数据，避免两栏颜色不一致。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::*;

use crate::theme::{TelaTheme, ThemeColor};

/// 从 `gpui_kit::assets::IconName::ALL` 看，这是一个很便宜的映射（约 1800 项），
/// 只在第一次画图标时建立；之后按名字 O(1) 取。
static ICON_INDEX: OnceLock<HashMap<String, IconName>> = OnceLock::new();

/// 默认主题的共享句柄：没有安装过全局主题时（测试、最小视图）使用。
static DEFAULT_THEME: OnceLock<Arc<TelaTheme>> = OnceLock::new();

/// 主题全局。
#[derive(Clone)]
struct TelaThemeGlobal(Arc<TelaTheme>);

impl Global for TelaThemeGlobal {}

/// 读取当前主题的扩展方法。
pub trait ActiveTelaTheme {
    /// 当前主题（共享句柄，克隆成本是一次引用计数）。
    fn tela_theme(&self) -> Arc<TelaTheme>;
}

impl ActiveTelaTheme for App {
    fn tela_theme(&self) -> Arc<TelaTheme> {
        self.try_global::<TelaThemeGlobal>()
            .map(|global| Arc::clone(&global.0))
            .unwrap_or_else(default_theme)
    }
}

/// 默认主题（内置 `catppuccin-mocha`）。
pub fn default_theme() -> Arc<TelaTheme> {
    Arc::clone(DEFAULT_THEME.get_or_init(|| Arc::new(crate::theme::default_theme().clone())))
}

/// 安装/切换当前主题。调用方负责 `cx.notify()` 让窗口重绘。
pub fn install(cx: &mut App, theme: TelaTheme) {
    cx.set_global(TelaThemeGlobal(Arc::new(theme)));
}

/// 主题颜色 → GPUI 颜色（保留透明度）。
pub fn hsla(color: ThemeColor) -> Hsla {
    rgba(color.rgba()).into()
}

/// 按名字找 Lucide 图标。
///
/// 名字用图标文件名（kebab-case），也就是 <https://lucide.dev/icons> 上的名字；
/// `file_text` 这样的下划线写法也接受。找不到时返回 `None`，由调用方决定是回退成
/// 文字还是什么都不画。
pub fn resolve_icon(name: &str) -> Option<IconName> {
    let key = normalize_icon_name(name);
    if key.is_empty() {
        return None;
    }
    icon_index().get(&key).copied()
}

/// 画一个主题图标。
///
/// - 名字能在 Lucide 库里找到：画 SVG，颜色跟随 `color`；
/// - 找不到（用户写了 emoji / 单字）：按文本画，同样使用 `color` 与 `size`；
/// - 名字为空：什么都不画，调用方可以无脑拼在按钮前面。
pub fn icon_element(name: &str, color: Hsla, size: Pixels) -> AnyElement {
    let name = name.trim();
    if name.is_empty() {
        return div().into_any_element();
    }
    match resolve_icon(name) {
        Some(icon) => Icon::new(icon)
            .text_color(color)
            .size(size)
            .into_any_element(),
        None => div()
            .text_size(size)
            .text_color(color)
            .child(name.to_string())
            .into_any_element(),
    }
}

fn normalize_icon_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('_', "-")
}

fn icon_index() -> &'static HashMap<String, IconName> {
    ICON_INDEX.get_or_init(|| {
        let mut index = HashMap::with_capacity(IconName::ALL.len());
        for icon in IconName::ALL {
            let path = icon.path();
            let key = path
                .strip_prefix("icons/")
                .and_then(|path| path.strip_suffix(".svg"))
                .unwrap_or(&path);
            index.insert(key.to_ascii_lowercase(), *icon);
        }
        index
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // `use gpui_kit::*` 会在 test-support 下把 gpui 的 `test` 宏带进作用域，
    // 这里显式使用标准库的 `#[test]`，避免递归展开。
    #[::core::prelude::v1::test]
    fn icon_names_resolve_both_spellings() {
        assert!(resolve_icon("rss").is_some());
        assert!(resolve_icon("file_text").is_some());
        assert!(resolve_icon("File-Text").is_some());
        assert!(resolve_icon("not-an-icon").is_none());
        assert!(resolve_icon("  ").is_none());
    }
}
