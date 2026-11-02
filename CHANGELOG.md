# 更新日志

本项目的版本发布见 [GitHub Releases](https://github.com/zhuangbiaowei/telaread-desktop/releases)。
下面按版本记录桌面端的主要变化；更细的提交历史见 `git log`。

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
