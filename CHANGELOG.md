# 更新日志

本项目的版本发布见 [GitHub Releases](https://github.com/zhuangbiaowei/telaread-desktop/releases)。
下面按版本记录桌面端的主要变化；更细的提交历史见 `git log`。

## v0.4.5 — 2026-11-03

**主题**：图标、字体与颜色、边框修饰，一组就是一个主题。

- 侧栏底部新增「主题 · 当前主题名」入口；主题选择器列出内置 **Catppuccin Mocha**
  （深色，默认）、**Catppuccin Latte**（浅色）与 **Paper 纸页**（暖色 + 衬线正文 + 书页图标）。
- 主题分成三组配置：`[icons]`（博客、收藏、文章、推荐、按钮、打开原文、缓存、新增、
  移除、主题入口）、`[typography]` + `[colors]`（界面/正文/标题/代码字体，字号、行高、
  段落间距，以及标题/链接/代码/引用/选区等颜色）、`[borders]`（描边、分隔线、焦点）。
- 本机主题：`<数据目录>/themes/<id>.toml`，文件名即 id；与内置主题同 id 时覆盖内置主题。
  选择器里的「重新扫描」支持边改边看；坏文件只会在选择器底部报错，不会阻止启动。
- 选择立即生效并写入 `<数据目录>/ui.toml`，开窗前解析，首帧就是上次的主题；
  主题不参与账号同步。`TELA_THEME` / `TELA_THEMES_DIR` 可用于临时切换主题与主题目录。
- 阅读正文的字体、字号、行高、段落间距、标题层级与链接/代码/引用/选区颜色全部来自主题；
  侧栏、列表、阅读面板与两个弹窗的边框/底色/文字色也不再写死。
- 完整格式见 [docs/themes.md](./docs/themes.md)。

**发布打包**：macOS 改成 `Tela.app`，Linux 增加 AppImage。

- macOS 包从「tar.gz 里两个裸可执行文件」改成 zip 里的 `Tela.app`
  （`packaging/macos/Info.plist` + 图标），并做 ad-hoc 签名（`codesign -s -`）：
  有 Dock 图标和菜单栏，双击才是正常应用。首次打开仍要在「系统设置 → 隐私与安全性」
  点「仍要打开」，因为 ad-hoc 签名满足不了 Gatekeeper（macOS 15 起也没有
  「右键 → 打开」这条绕过了）。彻底免掉这一步需要 Developer ID 签名 + 公证，本轮不做。
- Linux 新增 AppImage（`packaging/linux/` 的 `AppRun` + `.desktop` + 图标），
  与 tar.gz 并存；`release.yml` 打完包会用 `--check --demo` 冒烟验证产物。
- Windows 包不变。

**本地安装**：release 构建可以直接装成桌面入口。

- 新增 `packaging/linux/install-desktop-entry.sh`：构建 `--release --features gui`，
  并把启动脚本、`.desktop`、图标装到 `~/.local/{bin,share}`，装完在 Omarchy 里
  `SUPER + SPACE` → `tela` 即可启动（`--no-build` / `--uninstall` 只重装或移除入口）。
- 启动逻辑从用户目录搬进仓库（`packaging/linux/tela-desktop-launcher.sh` 模板）：
  探测本地代理、读 `tmp/tela.env`、日志写 `~/.local/state/tela-desktop.log`、
  缺二进制时自动构建；仓库路径在安装时写入，也可用 `TELA_PROJECT_ROOT` 覆盖。

## v0.4.4 — 2026-11-02

**代理支持**：HTTP 客户端读取环境变量里的代理设置。

- 启用 reqwest 的 `system-proxy`（此前为了只用 rustls 关掉了默认特性，
  连带关掉了环境代理）：`http_proxy` / `https_proxy` / `all_proxy` / `no_proxy`
  （大小写均可）对真实 API 的登录、同步与网页收藏抓取同时生效，
  不需要改代码或重编。`Cargo.lock` 相应引入 `system-configuration 0.7.0`（macOS 分支）。
- README 补充代理变量说明、本地代理用法示例，以及启动脚本的探测规则
  （`TELA_PROXY` / `TELA_PROXY_CANDIDATES`；`TELA_PROXY=off` 强制直连）。

## v0.4.3 — 2026-11-02

**网页收藏（`Shift+B`）**：把任意网址保存为本地可离线阅读的文章。

- 任意界面按 `Shift+B` 打开「收藏网页」弹窗：单行网址输入（剪贴板里是网址时自动预填）、
  `Esc` 取消、`Enter`/按钮提交；下载在后台线程执行，不阻塞界面。
- 正文以 **Markdown** 为规范格式：
  `dom_smoothie`（Readability.js 移植）抽取正文/标题/站点/语言 → 本地清洗
  （隐藏节点、懒加载图片、白名单标签、绝对地址）→ `htmd`（turndown.js 移植）转 Markdown；
  阅读时用 `pulldown-cmark` 把 Markdown 渲染回块 HTML。
- 图片按原始绝对地址下载进 `cached_images`，Markdown 里保留链接；离线浏览时从本地缓存读取。
- 本地库 schema v6：新增 `documents`（Markdown 正文 + 语言）；`bookmarks` 继续保存
  URL、标题、站点、摘要与图片数。旧版本写入 `bodies` 的块正文仍可回退读取。
- 站点适配：
  - 微信公众平台用微信内 UA + `#js_content`，避免被重定向到「环境异常」验证页；
  - X（Twitter）从 SSR 数据取长推文全文与引用帖，标题取正文第一行；
  - X 长文章（`/<user>/article/<id>`，以及同一篇的 `/<user>/status/<id>`）按
    `article_results.content_state` 还原文章标题、段落/小标题/列表/链接/图片；
    `t.co` 文章短链对匿名客户端会落到登录页，返回可操作的 `TelaError::Blocked`；
  - 图片识别 `data-src` / `data-original` / `data-lazy-src` 等懒加载属性；
  - `display:none` / `hidden` 节点整棵跳过（微信小标题的 `unset` 标记不再出现）；
  - 验证页/登录墙会返回 `TelaError::Blocked`，不再把验证页存成文章。
- 侧栏「我的推荐」下方新增「我的收藏」（显示条数），中间栏列出标题/站点/日期/图片数/网址，
  行内「移除」删除收藏；列表右上角有「＋ 收藏」入口。
- 文档与验收：S8 截图、`webpage`/`markdown`/`store_v2`/`services_v2`/`gui_shell` 测试、
  README（中/英）与 `docs/gui_acceptance.md` 同步更新。

## v0.4.2 — 2026-11-01

- 阅读面板改为整篇连续滚动，不再按段落分页；`PgDn`/`↓` 一路读到底，文末有「全文完」标记。
- 新增英文 README。

## v0.4.1 — 2026-10-31

- 推荐与「我的推荐」：阅读面板「推荐」弹窗（推荐语 ≤500 字，可编辑/撤回），
  推荐先写本地库并入离线队列，侧栏「我的推荐」按时间倒序展示。
- 只读（`read` scope）或 demo 装配下写操作返回明确错误，不伪装成功。

## v0.4.0 — 2026-10-30

- 本地优先内容层：正文/译文/图片首次取回后写 SQLite，之后离线阅读；
  博客名旁的「缓存」按钮按订阅批量预取，断网自动暂停译文轮询并低频探活。

## v0.3.3 — 2026-10-29

- 文章列表显示发布日期；阅读区标题旁 `↗` 用系统浏览器打开原文。
- release 工作流增加 macOS（Apple Silicon / Intel）原生包。
