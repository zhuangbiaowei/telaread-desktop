# 主题

Tela 的主题是一组**本机界面配置**：图标、字体与颜色、边框修饰。一组这样的配置
就是一个主题；切换主题只影响当前设备，不参与账号同步，也不改文章内容。

主题入口在侧栏底部：

```text
主题 · Catppuccin Mocha      切换
```

点开后是主题选择器，列出内置主题与 `<数据目录>/themes/` 下的本机主题。
选择后立即生效并写进 `<数据目录>/ui.toml`，下次启动第一帧就是该主题。

## 内置主题

| id | 名称 | 明暗 | 说明 |
|---|---|---|---|
| `catppuccin-mocha` | Catppuccin Mocha | 深色 | 默认主题，与旧版界面配色一致 |
| `catppuccin-latte` | Catppuccin Latte | 浅色 | Catppuccin 的浅色变体 |
| `paper` | Paper 纸页 | 浅色 | 暖色纸页 + 衬线正文 + 书页图标，演示三组配置一起换 |

主题 id 取文件名（不含 `.toml`）；与内置主题同 id 时会覆盖内置的那一份，
方便在现有主题上只改几项。

## 主题目录

| 系统 | 数据目录 | 主题文件 |
|---|---|---|
| Linux | `$XDG_DATA_HOME/tela/` 或 `~/.local/share/tela/` | `<数据目录>/themes/*.toml` |
| Windows | `%APPDATA%\tela\` | `<数据目录>\themes\*.toml` |
| macOS / 其他 | 由启动时的数据库路径决定（与 `tela.db` 同目录） | `<数据目录>/themes/*.toml` |

选中的主题保存在 `<数据目录>/ui.toml`：

```toml
# Tela 界面设置（本机保存，不参与同步）
theme = "catppuccin-latte"
```

两个环境变量用于开发/调试：

- `TELA_THEMES_DIR`：覆盖主题目录（例如指向仓库里的 `crates/tela-ui/themes`）；
- `TELA_THEME`：临时强制使用某个主题，**不写回** `ui.toml`。

## 主题文件格式

文件名（不含 `.toml`）就是主题 id。例如 `themes/mine.toml` 的 id 是 `mine`。
所有字段都可以省略，省略时沿用默认值（即 `catppuccin-mocha` 的配色与排版）：

```toml
name = "我的主题"
appearance = "dark"          # dark | light

[icons]
# 每项是 Lucide 图标名（https://lucide.dev/icons），也可以写一个字/emoji；
# 留空表示不显示。颜色跟随所在控件，不需要在主题里指定。
feed = "rss"                 # 博客/订阅
bookmark = "bookmark"        # 收藏
article = "file-text"        # 文章
recommendation = "star"      # 推荐
button = "sparkles"          # 普通按钮（没有专属图标的操作）
external = "external-link"   # 打开原文
cache = "download"           # 离线缓存
add = "plus"                 # 新增
remove = "circle-x"          # 移除/取消
refresh = "refresh-cw"       # 刷新/检查网页更新
theme = "palette"            # 主题入口

[typography]
ui_font = "Noto Sans CJK SC"        # 界面字体；留空用系统默认
body_font = "DejaVu Serif"          # 正文字体
heading_font = "DejaVu Serif"       # 标题字体；留空跟随正文字体
mono_font = "monospace"             # 代码字体
body_size = 17.5                    # 正文字号（px，10–32）
line_height = 1.8                   # 行高（字号倍数，1.0–3.0）
paragraph_gap = 0.6                 # 段落间距（rem，0–3）
heading_scale = 1.05                # 标题整体缩放（0.8–1.6）

[colors]
background = "#f6f1e7"      # 窗口底色
sidebar = "#efe7d8"         # 侧栏底色
surface = "#fbf7ef"         # 卡片/列表/标题栏
text = "#3f3a33"            # 主文字
muted = "#8c8273"           # 次要文字
subtle = "#6b6154"          # 比主文字弱一档
accent = "#9a5b32"          # 强调色
danger = "#b3382c"          # 错误
warning = "#a6751f"         # 警告
on_accent = "#fbf7ef"       # 强调色底上的文字

# 文章排版颜色
heading = "#2f2a24"         # 标题
link = "#8a4b23"            # 链接
code_text = "#3f3a33"       # 行内代码文字
code_background = "#ece4d4" # 代码块/行内代码底色
quote = "#6b6154"           # 引用块
selection = "#d9b98c88"     # 选中文字的背景（可带透明度）
hover = "#efe7d8"           # 悬停底色
selected = "#e3d6bf"        # 选中行底色

[borders]
border = "#d8c9b0"          # 卡片/按钮/输入框描边
divider = "#e3d6bf"         # 区域分隔线
focus = "#9a5b32"           # 焦点/当前主题高亮
```

### 颜色

`#RRGGBB` 或 `#RRGGBBAA`，`#` 可省略，大小写不限。`RRGGBBAA` 的透明度用于
选区、半透明高亮等场景。

### 字体

字体名必须与系统里安装的字体一致（字体管理器里显示的名字）。例如
`fc-list : family` 可以列出 Linux 上的字体。字体不存在时 GPUI 会回退到系统
默认字体，界面不会坏，但排版可能不是你想要的效果。`body_font` 为 None 时
正文使用界面字体，`heading_font` 为 None 时跟随正文。

### 图标

图标值先按 Lucide 的图标文件名（kebab-case，如 `file-text`、`external-link`）
查找；找不到时会把该字符串当普通文字画出来，因此也可以直接写 `📚`、`→`。
名字为空则不占位。用 `theme` 这一项控制侧栏底部「主题」入口的图标。

## 出错了怎么办

主题文件解析失败不会阻止启动。问题会显示在主题选择器底部，例如：

```text
1 个主题文件未加载：
/home/you/.local/share/tela/themes/mine.toml：主题格式有误：...
```

常见原因：

- 颜色少了 `#` 或位数不对（只接受 6/8 位十六进制）；
- 字号/行高/标题缩放超出上面括号里的范围；
- 字段名拼错（Tela 会拒绝未知字段，避免改了却没生效）。

改完文件后点主题选择器里的「重新扫描」，不用重启；选中的主题文件被删除时，
会自动回到 `catppuccin-mocha`。

## 实现位置

- 纯数据模型、内置主题、文件加载与 `ui.toml` 读写在
  [`crates/tela-ui/src/theme.rs`](../crates/tela-ui/src/theme.rs)；
- GUI 全局 `TelaTheme` 与 Lucide 图标解析在
  [`crates/tela-ui/src/gui/theme.rs`](../crates/tela-ui/src/gui/theme.rs)；
- 内置主题文件在 [`crates/tela-ui/themes/`](../crates/tela-ui/themes/)。
