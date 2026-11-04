//! 主题：一组「图标 + 排版字体与颜色 + 边框修饰」= 一个主题。
//!
//! 主题是**本机设置**，不参与云端同步（换设备、换账号都应保留各自的界面偏好）。
//! 数据来源有两层：
//!
//! 1. 编译进二进制的内置主题（`crates/tela-ui/themes/*.toml`）；
//! 2. `<数据目录>/themes/*.toml` 里的用户主题，文件名（不含扩展名）就是主题 id；
//!    与内置主题同 id 时覆盖内置的那一份，方便在现有配色上改两个字。
//!
//! 当前选中的主题写在 `<数据目录>/ui.toml` 的 `theme` 字段；`TELA_THEME`
//! 环境变量可以临时覆盖它（不写回文件）。`TELA_THEMES_DIR` 可以改主题目录，
//! 方便开发时把主题放在仓库里调试。
//!
//! 主题文件解析失败不会阻止启动：坏文件被收集进 [`ThemeCatalog::issues`]，
//! 在主题选择器里显示原因，应用照常使用当前/默认主题。这样用户写错一个颜色
//! 也不会把窗口弄没。

use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 覆盖主题目录（`TELA_THEMES_DIR`）。
pub const ENV_THEMES_DIR: &str = "TELA_THEMES_DIR";
/// 临时覆盖当前主题（`TELA_THEME`），不写回 `ui.toml`。
pub const ENV_THEME: &str = "TELA_THEME";
/// 默认主题 id；也是主题文件全部字段缺省时的取值来源。
pub const DEFAULT_THEME_ID: &str = "catppuccin-mocha";
/// 用户主题与内置主题的文件扩展名。
pub const THEME_EXTENSION: &str = "toml";
/// 主题选择保存的文件名（放在数据目录下）。
pub const SETTINGS_FILE: &str = "ui.toml";

/// 内置主题：`(id, 文件内容)`。第一个必须是 [`DEFAULT_THEME_ID`]。
const BUILTIN_THEMES: &[(&str, &str)] = &[
    (
        DEFAULT_THEME_ID,
        include_str!("../themes/catppuccin-mocha.toml"),
    ),
    (
        "catppuccin-latte",
        include_str!("../themes/catppuccin-latte.toml"),
    ),
    ("paper", include_str!("../themes/paper.toml")),
];

/// 一个颜色：`#RRGGBB` 或 `#RRGGBBAA`，内部按 `0xRRGGBBAA` 存放。
///
/// 主题层不依赖 GPUI，因此这里只做解析与存储；GUI 层再转成 `Hsla`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThemeColor(u32);

impl ThemeColor {
    /// 直接用 `0xRRGGBBAA` 构造。
    pub const fn from_rgba(rgba: u32) -> Self {
        Self(rgba)
    }

    /// 解析 `#RRGGBB` / `#RRGGBBAA`（`#` 可省略，大小写不限）。
    pub fn parse(input: &str) -> Result<Self, String> {
        let text = input.trim().trim_start_matches('#');
        if !matches!(text.len(), 6 | 8) || !text.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("颜色格式应为 #RRGGBB 或 #RRGGBBAA，收到 {input:?}"));
        }
        let mut value =
            u32::from_str_radix(text, 16).map_err(|_| format!("无法解析颜色 {input:?}"))?;
        if text.len() == 6 {
            // 六位是常见的「不带透明度」写法；补成不透明。
            value = (value << 8) | 0xff;
        }
        Ok(Self(value))
    }

    /// `0xRRGGBBAA`。
    pub const fn rgba(self) -> u32 {
        self.0
    }

    /// `0xRRGGBB`（丢掉透明度）。
    pub const fn rgb(self) -> u32 {
        self.0 >> 8
    }

    /// `0..=255` 的透明度。
    pub const fn alpha(self) -> u8 {
        (self.0 & 0xff) as u8
    }

    /// 是否完全不透明。
    pub const fn is_opaque(self) -> bool {
        self.alpha() == 0xff
    }
}

impl fmt::Display for ThemeColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_opaque() {
            write!(f, "#{:06x}", self.rgb())
        } else {
            write!(f, "#{:08x}", self.0)
        }
    }
}

impl Serialize for ThemeColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ThemeColor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// 主题的明暗类型；用于选择列表里的标注、富文本的深色资源变体。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// 深色。
    #[default]
    Dark,
    /// 浅色。
    Light,
}

impl Appearance {
    /// 中文标签（主题选择器与文档用）。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dark => "深色",
            Self::Light => "浅色",
        }
    }

    /// 是否深色。
    pub const fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }

    /// 是否浅色。
    pub const fn is_light(self) -> bool {
        matches!(self, Self::Light)
    }
}

/// 图标：每项是 Lucide 图标名（如 `"rss"`，见 <https://lucide.dev/icons>），
/// 也可以是任意一个字/glyph/emoji（如 `"📚"`）；留空表示不显示。
///
/// 主题只描述「用哪个图标」，具体颜色跟随所在控件，所以换主题不会让图标
/// 在深色底上消失。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IconSet {
    /// 博客/订阅。
    pub feed: String,
    /// 收藏。
    pub bookmark: String,
    /// 文章。
    pub article: String,
    /// 推荐。
    pub recommendation: String,
    /// 普通按钮（没有专属图标的操作按钮）。
    pub button: String,
    /// 打开原文/外链。
    pub external: String,
    /// 离线缓存。
    pub cache: String,
    /// 新增。
    pub add: String,
    /// 移除/取消。
    pub remove: String,
    /// 刷新/检查更新。
    pub refresh: String,
    /// 主题/外观入口。
    pub theme: String,
}

impl Default for IconSet {
    fn default() -> Self {
        Self {
            feed: "rss".to_string(),
            bookmark: "bookmark".to_string(),
            article: "file-text".to_string(),
            recommendation: "star".to_string(),
            button: "sparkles".to_string(),
            external: "external-link".to_string(),
            cache: "download".to_string(),
            add: "plus".to_string(),
            remove: "circle-x".to_string(),
            refresh: "refresh-cw".to_string(),
            theme: "palette".to_string(),
        }
    }
}

/// 排版：界面/正文/标题/代码字体，以及正文尺寸与段落节奏。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Typography {
    /// 界面字体；留空用系统默认。
    pub ui_font: Option<String>,
    /// 正文字体；留空用系统默认。
    pub body_font: Option<String>,
    /// 标题字体；留空跟随正文字体。
    pub heading_font: Option<String>,
    /// 代码字体。
    pub mono_font: Option<String>,
    /// 正文字号（px）。
    pub body_size: f32,
    /// 正文行高（字号的倍数）。
    pub line_height: f32,
    /// 段落间距（rem）。
    pub paragraph_gap: f32,
    /// 标题整体缩放；1.0 保持阅读器的默认层级。
    pub heading_scale: f32,
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            ui_font: None,
            body_font: None,
            heading_font: None,
            mono_font: Some("monospace".to_string()),
            body_size: 17.0,
            line_height: 1.75,
            paragraph_gap: 0.55,
            heading_scale: 1.0,
        }
    }
}

impl Typography {
    /// 去掉字体名两侧空白，空串视为「用系统默认」。
    fn normalize(&mut self) {
        for font in [
            &mut self.ui_font,
            &mut self.body_font,
            &mut self.heading_font,
            &mut self.mono_font,
        ] {
            if let Some(value) = font {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    *font = None;
                } else if trimmed.len() != value.len() {
                    *font = Some(trimmed.to_string());
                }
            }
        }
    }
}

/// 颜色：界面底色、正文排版色与状态色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Palette {
    /// 窗口底色。
    pub background: ThemeColor,
    /// 侧栏底色。
    pub sidebar: ThemeColor,
    /// 卡片/列表/标题栏底色。
    pub surface: ThemeColor,
    /// 主文字色。
    pub text: ThemeColor,
    /// 弱化文字（次要信息）。
    pub muted: ThemeColor,
    /// 比主文字弱一档、但仍清晰的文字。
    pub subtle: ThemeColor,
    /// 强调色（选中、链接按钮、进度）。
    pub accent: ThemeColor,
    /// 危险/错误色。
    pub danger: ThemeColor,
    /// 警告色。
    pub warning: ThemeColor,
    /// 正文标题色。
    pub heading: ThemeColor,
    /// 正文链接色。
    pub link: ThemeColor,
    /// 行内代码文字色。
    pub code_text: ThemeColor,
    /// 代码块与行内代码底色。
    pub code_background: ThemeColor,
    /// 引用块文字色。
    pub quote: ThemeColor,
    /// 正文选区底色（可带透明度）。
    pub selection: ThemeColor,
    /// 鼠标悬停底色。
    pub hover: ThemeColor,
    /// 选中行/选中项的底色。
    pub selected: ThemeColor,
    /// 强调色底上的文字色。
    pub on_accent: ThemeColor,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            background: ThemeColor::from_rgba(0x11111bff),
            sidebar: ThemeColor::from_rgba(0x181825ff),
            surface: ThemeColor::from_rgba(0x1e1e2eff),
            text: ThemeColor::from_rgba(0xcdd6f4ff),
            muted: ThemeColor::from_rgba(0x6c7086ff),
            subtle: ThemeColor::from_rgba(0xa6adc8ff),
            accent: ThemeColor::from_rgba(0x89b4faff),
            danger: ThemeColor::from_rgba(0xf38ba8ff),
            warning: ThemeColor::from_rgba(0xf9e2afff),
            heading: ThemeColor::from_rgba(0xcdd6f4ff),
            link: ThemeColor::from_rgba(0x89b4faff),
            code_text: ThemeColor::from_rgba(0xcdd6f4ff),
            code_background: ThemeColor::from_rgba(0x1e1e2eff),
            quote: ThemeColor::from_rgba(0xa6adc8ff),
            selection: ThemeColor::from_rgba(0x89b4fa55),
            hover: ThemeColor::from_rgba(0x1e1e2eff),
            selected: ThemeColor::from_rgba(0x313244ff),
            on_accent: ThemeColor::from_rgba(0x11111bff),
        }
    }
}

/// 边框与修饰色：分隔线、卡片描边、焦点环。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Borders {
    /// 卡片、按钮、输入框的描边。
    pub border: ThemeColor,
    /// 区域之间的分隔线（比描边更轻）。
    pub divider: ThemeColor,
    /// 焦点/当前主题的高亮环。
    pub focus: ThemeColor,
}

impl Default for Borders {
    fn default() -> Self {
        Self {
            border: ThemeColor::from_rgba(0x313244ff),
            divider: ThemeColor::from_rgba(0x313244ff),
            focus: ThemeColor::from_rgba(0x89b4faff),
        }
    }
}

/// 主题的来源（只用于界面标注与「重新扫描」）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ThemeSource {
    /// 编译进二进制的内置主题。
    #[default]
    Builtin,
    /// 数据目录下的用户主题文件。
    File(PathBuf),
}

impl ThemeSource {
    /// 中文标签。
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Builtin => "内置",
            Self::File(_) => "本机",
        }
    }

    /// 用户主题的文件路径。
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Builtin => None,
            Self::File(path) => Some(path),
        }
    }
}

/// 一个主题：图标、排版（字体 + 颜色）、边框修饰。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelaTheme {
    /// 展示名。
    #[serde(default = "default_name")]
    pub name: String,
    /// 明暗类型。
    #[serde(default)]
    pub appearance: Appearance,
    /// 图标。
    #[serde(default)]
    pub icons: IconSet,
    /// 字体与字号。
    #[serde(default)]
    pub typography: Typography,
    /// 颜色。
    #[serde(default)]
    pub colors: Palette,
    /// 边框修饰。
    #[serde(default)]
    pub borders: Borders,
    /// 主题 id：内置主题写死，用户主题取文件名（不含 `.toml`）。
    #[serde(skip)]
    pub id: String,
    /// 来源。
    #[serde(skip)]
    pub source: ThemeSource,
}

fn default_name() -> String {
    "未命名主题".to_string()
}

impl Default for TelaTheme {
    fn default() -> Self {
        default_theme().clone()
    }
}

impl TelaTheme {
    /// 从 TOML 文本解析；`id` 由调用方给出（内置 id 或文件名）。
    pub fn from_toml(
        id: impl Into<String>,
        source: ThemeSource,
        text: &str,
    ) -> Result<Self, ThemeError> {
        let mut theme: Self = toml::from_str(text).map_err(|error| {
            ThemeError::new(
                source.path().map(Path::to_path_buf),
                format!("主题格式有误：{error}"),
            )
        })?;
        theme.id = id.into();
        if theme.name.trim().is_empty() {
            theme.name = theme.id.clone();
        }
        theme.source = source;
        theme.typography.normalize();
        theme.validate().map_err(|message| {
            ThemeError::new(
                theme.source.path().map(Path::to_path_buf),
                format!("主题「{}」{message}", theme.name),
            )
        })?;
        Ok(theme)
    }

    /// 数值范围校验：坏值会让排版彻底不可读，宁可报错让用户改。
    fn validate(&self) -> Result<(), String> {
        let typography = &self.typography;
        if !typography.body_size.is_finite() || !(10.0..=32.0).contains(&typography.body_size) {
            return Err(format!(
                "的正文字号 {} 超出范围（10–32 px）",
                typography.body_size
            ));
        }
        if !typography.line_height.is_finite() || !(1.0..=3.0).contains(&typography.line_height) {
            return Err(format!(
                "的行高 {} 超出范围（1.0–3.0）",
                typography.line_height
            ));
        }
        if !typography.paragraph_gap.is_finite() || !(0.0..=3.0).contains(&typography.paragraph_gap)
        {
            return Err(format!(
                "的段落间距 {} 超出范围（0–3 rem）",
                typography.paragraph_gap
            ));
        }
        if !typography.heading_scale.is_finite() || !(0.8..=1.6).contains(&typography.heading_scale)
        {
            return Err(format!(
                "的标题缩放 {} 超出范围（0.8–1.6）",
                typography.heading_scale
            ));
        }
        Ok(())
    }

    /// 是否深色主题。
    pub fn is_dark(&self) -> bool {
        self.appearance.is_dark()
    }

    /// 该主题是否来自用户主题文件。
    pub fn is_user_theme(&self) -> bool {
        matches!(self.source, ThemeSource::File(_))
    }
}

/// 默认主题（内置的 [`DEFAULT_THEME_ID`]）。
pub fn default_theme() -> &'static TelaTheme {
    static DEFAULT: OnceLock<TelaTheme> = OnceLock::new();
    DEFAULT.get_or_init(|| {
        let (id, text) = BUILTIN_THEMES
            .iter()
            .find(|(id, _)| *id == DEFAULT_THEME_ID)
            .expect("内置主题列表必须包含默认主题");
        TelaTheme::from_toml(*id, ThemeSource::Builtin, text)
            .expect("内置主题必须可解析（由 tests 保证）")
    })
}

/// 内置主题（每次调用都重新解析，保证与内存中的主题文件一致）。
pub fn builtin_themes() -> Vec<TelaTheme> {
    BUILTIN_THEMES
        .iter()
        .map(|(id, text)| {
            TelaTheme::from_toml(*id, ThemeSource::Builtin, text)
                .expect("内置主题必须可解析（由 tests 保证）")
        })
        .collect()
}

/// 主题相关的本地路径。
///
/// `None` 表示「不使用本机主题文件」：单元测试与只读装配只加载内置主题。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemePaths {
    /// 用户主题目录（`<数据目录>/themes`）。
    pub themes_dir: Option<PathBuf>,
    /// 当前主题选择文件（`<数据目录>/ui.toml`）。
    pub settings_path: Option<PathBuf>,
}

impl ThemePaths {
    /// 由数据目录推导：`<dir>/themes/` 与 `<dir>/ui.toml`。
    pub fn from_data_dir(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        Self {
            themes_dir: Some(dir.join("themes")),
            settings_path: Some(dir.join(SETTINGS_FILE)),
        }
    }

    /// 同 [`Self::from_data_dir`]，但允许 `TELA_THEMES_DIR` 覆盖主题目录。
    pub fn from_env_or_data_dir(dir: impl AsRef<Path>) -> Self {
        let mut paths = Self::from_data_dir(dir);
        if let Some(raw) = env::var_os(ENV_THEMES_DIR) {
            paths.themes_dir = Some(PathBuf::from(raw));
        }
        paths
    }
}

/// 一条主题加载/保存问题；在主题选择器里按清单展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeError {
    /// 出错的文件（设置文件或主题文件）。
    pub path: Option<PathBuf>,
    /// 人话说明。
    pub message: String,
}

impl ThemeError {
    /// 构造一条问题。
    pub fn new(path: Option<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            path,
            message: message.into(),
        }
    }
}

impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(path) => write!(f, "{}：{}", path.display(), self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// 可用的主题集合 + 当前选择 + 加载问题。
#[derive(Debug, Clone)]
pub struct ThemeCatalog {
    themes: Vec<TelaTheme>,
    active: String,
    paths: ThemePaths,
    issues: Vec<ThemeError>,
}

impl ThemeCatalog {
    /// 只加载内置主题（单元测试、无本机目录的装配）。
    pub fn builtin() -> Self {
        Self::load(&ThemePaths::default())
    }

    /// 加载内置主题 + 本机主题，并解析当前选择。
    pub fn load(paths: &ThemePaths) -> Self {
        let mut issues = Vec::new();
        let themes = scan_themes(paths, &mut issues);
        let active = match requested_theme(paths, &mut issues) {
            Some(id) if themes.iter().any(|theme| theme.id == id) => id,
            Some(id) => {
                issues.push(ThemeError::new(
                    paths.settings_path.clone(),
                    format!("找不到主题「{id}」，已使用默认主题"),
                ));
                fallback_theme_id(&themes)
            }
            None => fallback_theme_id(&themes),
        };
        Self {
            themes,
            active,
            paths: paths.clone(),
            issues,
        }
    }

    /// 所有主题（内置在前，用户主题按文件名排序在后）。
    pub fn themes(&self) -> &[TelaTheme] {
        &self.themes
    }

    /// 当前主题。
    pub fn active(&self) -> &TelaTheme {
        self.get(&self.active)
            .unwrap_or_else(|| self.themes.first().expect("主题目录永远至少包含内置主题"))
    }

    /// 当前主题 id。
    pub fn active_id(&self) -> &str {
        &self.active
    }

    /// 按 id 取主题。
    pub fn get(&self, id: &str) -> Option<&TelaTheme> {
        self.themes.iter().find(|theme| theme.id == id)
    }

    /// 本地路径。
    pub fn paths(&self) -> &ThemePaths {
        &self.paths
    }

    /// 主题目录。
    pub fn themes_dir(&self) -> Option<&Path> {
        self.paths.themes_dir.as_deref()
    }

    /// 加载问题（坏文件、设置文件读不了等）。
    pub fn issues(&self) -> &[ThemeError] {
        &self.issues
    }

    /// 选中一个主题；主题不存在时返回 `None`，选择保持不变。
    pub fn select(&mut self, id: &str) -> Option<&TelaTheme> {
        if !self.themes.iter().any(|theme| theme.id == id) {
            return None;
        }
        self.active = id.to_string();
        self.get(id)
    }

    /// 重新扫描本机主题目录；当前选择仍然存在就保留，否则退回默认主题。
    ///
    /// 不会重新读取 `ui.toml`：用户在界面里刚选的主题不能被磁盘上的旧值覆盖。
    pub fn reload(&mut self) {
        let mut issues = Vec::new();
        self.themes = scan_themes(&self.paths, &mut issues);
        if !self.themes.iter().any(|theme| theme.id == self.active) {
            self.active = fallback_theme_id(&self.themes);
        }
        self.issues = issues;
    }

    /// 把当前选择写回 `ui.toml`；没有配置设置文件时什么也不做。
    pub fn persist_active(&self) -> io::Result<()> {
        let Some(path) = &self.paths.settings_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string(&UiSettings {
            theme: Some(self.active.clone()),
        })
        .map_err(|error| io::Error::other(format!("无法序列化界面设置：{error}")))?;
        write_atomic(
            path,
            format!("# Tela 界面设置（本机保存，不参与同步）\n{contents}"),
        )
    }
}

/// `ui.toml` 的结构；以后的字号/行宽设置也可以放这里。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct UiSettings {
    theme: Option<String>,
}

/// 先扫内置，再扫用户目录；用户主题同 id 覆盖内置。
fn scan_themes(paths: &ThemePaths, issues: &mut Vec<ThemeError>) -> Vec<TelaTheme> {
    let mut themes = builtin_themes();
    let Some(dir) = &paths.themes_dir else {
        return themes;
    };
    if let Err(error) = fs::create_dir_all(dir) {
        issues.push(ThemeError::new(
            Some(dir.clone()),
            format!("无法创建主题目录：{error}"),
        ));
        return themes;
    }
    for path in theme_files(dir, issues) {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                issues.push(ThemeError::new(
                    Some(path.clone()),
                    format!("无法读取主题文件：{error}"),
                ));
                continue;
            }
        };
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            issues.push(ThemeError::new(
                Some(path.clone()),
                "主题文件名不是有效的 UTF-8".to_string(),
            ));
            continue;
        };
        match TelaTheme::from_toml(id, ThemeSource::File(path.clone()), &text) {
            Ok(theme) => {
                // 用户同名主题覆盖内置主题，但保留它在列表末尾的位置更新。
                themes.retain(|existing| existing.id != theme.id);
                themes.push(theme);
            }
            Err(error) => issues.push(error),
        }
    }
    themes
}

/// 主题目录里的 `.toml` 文件，按文件名排序（列表顺序稳定）。
fn theme_files(dir: &Path, issues: &mut Vec<ThemeError>) -> Vec<PathBuf> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            issues.push(ThemeError::new(
                Some(dir.to_path_buf()),
                format!("无法读取主题目录：{error}"),
            ));
            return Vec::new();
        }
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(THEME_EXTENSION))
        })
        .collect();
    files.sort();
    files
}

/// 当前选择的来源：`TELA_THEME` 优先于 `ui.toml`。
fn requested_theme(paths: &ThemePaths, issues: &mut Vec<ThemeError>) -> Option<String> {
    if let Some(raw) = env::var_os(ENV_THEME) {
        let value = raw.to_string_lossy().trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }
    let path = paths.settings_path.as_ref()?;
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            issues.push(ThemeError::new(
                Some(path.clone()),
                format!("无法读取界面设置：{error}"),
            ));
            return None;
        }
    };
    match toml::from_str::<UiSettings>(&text) {
        Ok(settings) => settings.theme.filter(|id| !id.trim().is_empty()),
        Err(error) => {
            issues.push(ThemeError::new(
                Some(path.clone()),
                format!("界面设置格式有误：{error}"),
            ));
            None
        }
    }
}

fn fallback_theme_id(themes: &[TelaTheme]) -> String {
    themes
        .iter()
        .find(|theme| theme.id == DEFAULT_THEME_ID)
        .or_else(|| themes.first())
        .map(|theme| theme.id.clone())
        .unwrap_or_else(|| DEFAULT_THEME_ID.to_string())
}

/// 先写临时文件再改名，避免写到一半留下半个 `ui.toml`。
fn write_atomic(path: &Path, contents: String) -> io::Result<()> {
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, contents)?;
    // Windows 的 rename 不能覆盖已存在的目标文件；设置文件很小，先删再改名。
    #[cfg(windows)]
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let dir = env::temp_dir().join(format!("tela-theme-{tag}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn builtin_themes_parse_and_cover_the_three_groups() {
        let themes = builtin_themes();
        assert_eq!(themes.len(), BUILTIN_THEMES.len());
        assert_eq!(themes[0].id, DEFAULT_THEME_ID);
        for theme in &themes {
            assert!(!theme.name.is_empty());
            assert!(!theme.icons.feed.is_empty());
            assert!(theme.typography.body_size >= 10.0);
            assert!(theme.typography.line_height >= 1.0);
        }
        assert!(themes.iter().any(|theme| theme.is_dark()));
        assert!(themes.iter().any(|theme| !theme.is_dark()));
    }

    #[test]
    fn default_theme_matches_the_first_builtin() {
        let defaults = builtin_themes();
        assert_eq!(defaults[0].id, default_theme().id);
        assert_eq!(&defaults[0], default_theme());
    }

    #[test]
    fn color_parsing_accepts_rgb_and_rgba() {
        assert_eq!(ThemeColor::parse("#89b4fa").unwrap().rgb(), 0x89b4fa);
        assert!(ThemeColor::parse("#89B4FA").unwrap().is_opaque());
        assert_eq!(ThemeColor::parse("89b4fa55").unwrap().alpha(), 0x55);
        assert_eq!(
            ThemeColor::parse("#89b4fa55").unwrap().to_string(),
            "#89b4fa55"
        );
        assert!(ThemeColor::parse("#12345").is_err());
        assert!(ThemeColor::parse("blue").is_err());
    }

    #[test]
    fn partial_theme_inherits_defaults() {
        let theme = TelaTheme::from_toml(
            "mine",
            ThemeSource::Builtin,
            r##"
            name = "Mine"
            [colors]
            text = "#ffffff"
            "##,
        )
        .expect("partial theme parses");
        assert_eq!(theme.id, "mine");
        assert_eq!(theme.colors.text.to_string(), "#ffffff");
        assert_eq!(theme.colors.background, Palette::default().background);
        assert_eq!(theme.icons.feed, IconSet::default().feed);
        assert_eq!(theme.typography.body_size, Typography::default().body_size);
    }

    #[test]
    fn invalid_values_are_reported_not_panicking() {
        let color = TelaTheme::from_toml(
            "bad",
            ThemeSource::Builtin,
            "[colors]\nbackground = \"nope\"\n",
        );
        assert!(color.unwrap_err().message.contains("颜色"));

        let size = TelaTheme::from_toml(
            "bad",
            ThemeSource::Builtin,
            "[typography]\nbody_size = 100.0\n",
        );
        assert!(size.unwrap_err().message.contains("字号"));

        let unknown = TelaTheme::from_toml(
            "bad",
            ThemeSource::Builtin,
            "[colors]\nbackgronud = \"#000000\"\n",
        );
        assert!(unknown.is_err());
    }

    #[test]
    fn user_themes_load_from_disk_and_override_builtin() {
        let dir = temp_dir("user");
        let paths = ThemePaths {
            themes_dir: Some(dir.join("themes")),
            settings_path: Some(dir.join("ui.toml")),
        };
        fs::create_dir_all(paths.themes_dir.as_ref().unwrap()).unwrap();
        fs::write(
            paths.themes_dir.as_ref().unwrap().join("mine.toml"),
            "name = \"Mine\"\n[colors]\naccent = \"#ff0000\"\n",
        )
        .unwrap();
        // 与内置主题同 id：应当覆盖内置的 mocha。
        fs::write(
            paths
                .themes_dir
                .as_ref()
                .unwrap()
                .join("catppuccin-mocha.toml"),
            "name = \"Mocha Fork\"\n[icons]\nfeed = \"book-open\"\n",
        )
        .unwrap();
        // 坏文件：不阻止启动，只记一条 issue。
        fs::write(
            paths.themes_dir.as_ref().unwrap().join("broken.toml"),
            "name = ",
        )
        .unwrap();

        let catalog = ThemeCatalog::load(&paths);
        assert!(catalog.get("mine").is_some());
        let fork = catalog.get(DEFAULT_THEME_ID).unwrap();
        assert_eq!(fork.name, "Mocha Fork");
        assert_eq!(fork.icons.feed, "book-open");
        assert_eq!(fork.source.label(), "本机");
        assert!(catalog.issues().iter().any(|issue| {
            issue
                .path
                .as_ref()
                .is_some_and(|path| path.ends_with("broken.toml"))
        }));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn active_theme_is_persisted_and_restored() {
        let dir = temp_dir("persist");
        let paths = ThemePaths::from_data_dir(&dir);
        let mut catalog = ThemeCatalog::load(&paths);
        assert_eq!(catalog.active_id(), DEFAULT_THEME_ID);
        assert!(catalog.select("catppuccin-latte").is_some());
        catalog.persist_active().unwrap();
        assert!(dir.join(SETTINGS_FILE).exists());

        let reloaded = ThemeCatalog::load(&paths);
        assert_eq!(reloaded.active_id(), "catppuccin-latte");
        assert_eq!(reloaded.active().name, "Catppuccin Latte");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_theme_falls_back_to_default() {
        let dir = temp_dir("missing");
        fs::write(dir.join(SETTINGS_FILE), "theme = \"does-not-exist\"\n").unwrap();
        let catalog = ThemeCatalog::load(&ThemePaths::from_data_dir(&dir));
        assert_eq!(catalog.active_id(), DEFAULT_THEME_ID);
        assert!(
            catalog
                .issues()
                .iter()
                .any(|issue| issue.message.contains("does-not-exist"))
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reload_keeps_selection_and_drops_deleted_theme() {
        let dir = temp_dir("reload");
        let paths = ThemePaths::from_data_dir(&dir);
        fs::create_dir_all(paths.themes_dir.as_ref().unwrap()).unwrap();
        let file = paths.themes_dir.as_ref().unwrap().join("mine.toml");
        fs::write(&file, "name = \"Mine\"\n").unwrap();

        let mut catalog = ThemeCatalog::load(&paths);
        assert!(catalog.select("mine").is_some());
        catalog.reload();
        assert_eq!(catalog.active_id(), "mine");

        fs::remove_file(&file).unwrap();
        catalog.reload();
        assert_eq!(catalog.active_id(), DEFAULT_THEME_ID);
        fs::remove_dir_all(dir).ok();
    }
}
