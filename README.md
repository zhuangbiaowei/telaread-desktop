# Tela Desktop

简体中文 | [English](./README.en.md)

基于 [Tela](https://telaread.com) 的桌面阅读器（Rust + [GPUI Kit](https://github.com/longbridge/gpui-kit)）。

> **当前进度（v0.4.4）**：**真实 API v1 底座已接通** —— OAuth 2.1 + PKCE 登录、刷新令牌进系统凭据库、
> 全量/增量同步、离线变更队列、正文/译文/图片本地优先阅读、订阅级一键离线缓存、
> 推荐文章与「我的推荐」、`Shift+B` 收藏任意网页到「我的收藏」、断网自动暂停译文轮询、
> 终端 CLI 与离线 GUI demo。
> 协议依据是官方 [authorization.md](https://github.com/hutusi/tela/blob/main/docs/api/authorization.md)
> 与 [v1.md](https://github.com/hutusi/tela/blob/main/docs/api/v1.md)（2026-10-28 实读）。
> 详细落地说明见 [`docs/real-api.md`](./docs/real-api.md)。

## 快速开始（真实 API）

前置：在 Tela 注册一个 public native client，redirect URI 登记为 `http://127.0.0.1/callback`
（不带端口，运行时用任意空闲端口）。

```bash
export TELA_CLIENT_ID=tela_app_…
# 需要邮箱时：export TELA_SCOPES="write email"

cargo run -p tela-app -- login          # 打开系统浏览器，回环端口接收授权码
cargo run -p tela-app -- sync           # 先推离线队列，再拉最新数据
cargo run -p tela-app -- list           # 订阅列表 + 未读数
cargo run -p tela-app -- list --feed 1  # 某个订阅下的文章
cargo run -p tela-app -- read 101       # 阅读（打开即已读；--no-mark-read 可关闭）
cargo run -p tela-app -- mark-read 101  # 标记已读（入离线队列并立即推送）
cargo run -p tela-app -- mark-unread 101
cargo run -p tela-app -- like 101       # 点赞；--unlike 取消
cargo run -p tela-app -- status         # 游标、行数、待发队列、缓存数、今日额度
cargo run -p tela-app -- logout         # 撤销该设备令牌并清本地账号数据
```

不需要任何环境变量也能跑离线演示：

```bash
cargo run -p tela-app                    # 默认进入进程内 mock 的对照阅读页
cargo run -p tela-app -- demo --chapter mock-long
```

`tela-app --help` 列出全部子命令；`status --offline` 完全不访问网络。

### 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `TELA_CLIENT_ID` | 构建期内置 | 发布包已内置 public client id；本地构建/调试可覆盖 |
| `TELA_SCOPES` | `write` | `read` / `write`，可附加 `email`；`read` 令牌不能标记已读/点赞 |
| `TELA_API_BASE_URL` | `https://api.telaread.com` | API 基址 |
| `TELA_AUTH_BASE_URL` | `https://telaread.com` | 授权页站点 |
| `TELA_DB` | 见下方说明 | SQLite 路径 |
| `TELA_LANG` | `zh-Hans` | GUI 装配的默认译文语言；CLI 用全局 `--lang`（未指定时 `zh-Hans`） |
| `TELA_TOKEN_STORE` | `keyring` | `memory` / `ephemeral` 仅用于测试或临时会话，退出即失效 |
| `TELA_DEMO` | （未设置） | GUI 默认真实 API（登录模式）；`1`/`true`/`yes`/`on` 进入 demo |
| `TELA_DEV_SEED` | （未设置） | demo 装配且空库时是否写演示数据；`0` 关闭 |
| `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` | 继承环境 | 标准代理变量（大小写均可），真实 API 与网页抓取请求都会走代理 |

#### 走本地代理

HTTP 客户端读环境里的 `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY` / `NO_PROXY`
（大小写均可），所以不需要改代码就能让 Tela 走本地代理：

```bash
HTTPS_PROXY=http://127.0.0.1:7888 tela-desktop
```

`~/.local/bin/tela-desktop` 启动脚本会自动探测本机代理：先试 `TELA_PROXY`，
再依次试 `TELA_PROXY_CANDIDATES`（默认 `127.0.0.1:7888`、`7890`、`1080`）；
探测到可用代理就导出代理变量，探测不到就直连。探测结果写进
`~/.local/state/tela-desktop.log`。`TELA_PROXY=off` 可强制直连。

`TELA_DB` 默认路径按平台选择：`$XDG_DATA_HOME/tela/tela.db`（如果设置了
`XDG_DATA_HOME`）；Windows 为 `%APPDATA%\tela\tela.db`；否则
`$HOME/.local/share/tela/tela.db`。

## 终端 CLI

`tela-app --help` 是权威用法；所有子命令都支持三个全局参数：

| 参数 | 默认 | 说明 |
|---|---|---|
| `--lang` | `zh-Hans` | 译文语言：`zh-Hans` / `zh-Hant` / `en` / `fr` |
| `--view` | `compare` | 阅读视图：`original` / `compare` / `translation` |
| `--page-size` | `20` | 每页段落数 |

| 子命令 | 常用参数 | 说明 |
|---|---|---|
| `login` | `--no-browser` / `--ephemeral` | 浏览器授权；不自动开浏览器或只把刷新令牌放内存 |
| `logout` | — | 撤销设备令牌并清本地账号数据（正文/译文/图片缓存保留） |
| `sync` | `--full` / `--types articles,states` | 推送离线队列后拉取；`--full` 丢游标重来 |
| `list` | `--feed <id>` / `--unread` | 列订阅；或列某个订阅的文章并只显示未读 |
| `read <id>` | `--translate` / `--no-mark-read` | 阅读文章；可先请求翻译，或关闭打开即已读 |
| `mark-read <id>` | — | 标记已读（入队并立即推送） |
| `mark-unread <id>` | — | 标记未读 |
| `like <id>` | `--unlike` | 点赞 / 取消点赞 |
| `status` | `--offline` | 本地游标、行数、待发队列、正文/译文/图片缓存数；在线时附额度 |
| `meta` | — | 匿名读取服务端能力与限额 |
| `demo` | `--chapter <key>` | 离线演示阅读页，不需要账号与网络 |

不带子命令直接运行时等同 `demo`。交互阅读页支持：`n`/`next` 下一页、`p`/`prev`
上一页、`r`/`retry` 重试、`o`/`original` 原文、`c`/`compare` 对照、
`t`/`translation` 译文、`q` 退出。

`demo --chapter` 接受正文 key，也接受场景别名：

- `mock` / `normal` / `mock-normal`
- `mock-long` / `long`
- `mock-empty` / `empty`
- `mock-partial` / `partial`
- `mock-no-translation` / `no-translation` / `untranslated`

## 构建与测试

前置：Rust stable（本机验证 `rustc 1.98.1` / `cargo 1.98.1`；workspace 声明 `rust-version = "1.92"`）。
`reqwest` 使用 rustls（aws-lc-rs），首次构建需要 `cmake` 与 C 编译器；Linux 还需要 `pkg-config`。

```bash
cargo build --workspace --locked                          # 构建
cargo test --workspace --locked                           # 全部测试（含真实 API fixture 契约）
cargo run -p tela-app -- --version                        # tela-app 0.4.4
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

以上 5 条与 CI（[`.github/workflows/ci.yml`](./.github/workflows/ci.yml)）逐条对应。

真实 API 的在线冒烟（不需要令牌）：

```bash
TELA_TOKEN_STORE=memory TELA_DB=tmp/smoke.db cargo run -p tela-app -- meta
```

## GUI

GUI 依赖默认**不编译**：`tela-gui` 需要显式 `--features gui`，没有 Wayland/Vulkan 系统库的机器
仍可跑上面的默认检查。

```bash
# 默认：真实 API（登录模式）。未登录时侧栏显示「登录 Tela」，登录后自动同步
cargo run -p tela-app --features gui --bin tela-gui --locked

# 演示数据（不需要账号）：--demo，空库自动写入 2 个订阅 / 4 篇文章
TELA_DB=target/tela-gui.db \
  cargo run -p tela-app --features gui --bin tela-gui --locked -- --demo

# 本地调试真实 API：临时覆盖 client id / 库路径
source tmp/tela.env                 # 本地 client id / DB 路径（不提交）
cargo run -p tela-app --features gui --bin tela-gui --locked
scripts/tela-gui-real.sh --check    # 无窗口数据冒烟（读 tmp/tela.env + 真实装配）
```

发布包在构建时把 public client id 编进二进制（`.github/workflows/release.yml` 用仓库变量
`TELA_CLIENT_ID` 注入），所以用户解压后默认就是登录模式；`TELA_CLIENT_ID` 环境变量仍可覆盖。

### 窗口与布局

- 默认窗口 1100×720，最小 880×560；标题为 `Tela Desktop`，关闭最后一个窗口即退出。
- 三栏：左侧订阅列表（260 px）、中间当前订阅的文章、「我的推荐」或「我的收藏」（340–360 px）、
  右侧阅读面板（自适应）。
- 左侧顶部是品牌「TelaRead」，点击用系统浏览器打开 <https://telaread.com>；下方是当前订阅状态。
- 订阅列表上方是「我的推荐」与「我的收藏」两个入口，各显示条目数；点击切换对应列表。
- 每个订阅行显示标题、未读数、feed URL 和「缓存」按钮，选中行高亮；点击整行切换订阅。
- 中间列表顶部显示当前订阅标题与文章数；每篇文章显示标题、未读标记、发布日期和摘要。
- 侧栏底部显示订阅数、未读总数与账号面板。
- 空订阅、列表加载失败会显示对应空态/错误；打开 TelaRead 主页失败也会在侧栏提示。
- 真实登录模式下，选中文章或切换订阅（自动显示第一篇）会立即标为已读：本地先变，
  再后台推送；受 `reader.mark_on_open` 控制。

### 阅读面板

- 未选择文章时显示「选择一篇文章开始阅读」；空章节、加载中、加载失败都有独立状态页。
- 标题旁的 `↗` 用系统浏览器打开原文；打开失败会在标题下方提示。
- 顶部显示译文状态（已就绪 / 翻译中 / 部分缺失 / 暂无译文）和段落总数，右侧是
  `原文 | 对照 | 译文` 三个视图按钮；不显示页码，也没有「上一页 / 下一页」。
- 对照视图按块左右对齐原文与译文；缺译文的块回退显示原文，并在状态里标出缺失段落数。
- 正文按块渲染 HTML（标题、引用、列表、表格、链接、代码块、图片），文字可选中。
- 文章一次渲染整篇、连续滚动：按住 `PgDn` 或 `↓` 一路按到底就能读完，结尾有「全文完」标记；
  `PgUp` / `↑` 反向滚动，不会在页边界跳转或重置位置。
- 没有译文时会自动向服务端请求并轮询；有译文后写回本地缓存，后续打开不再请求。
  今日额度用完或作者关闭翻译时，会在标题下方给出对应提示并继续显示原文。
- 切换视图会保持当前阅读位置；加载失败显示错误详情与「重试」按钮，快捷键 `r` 等效。

### 推荐

- 阅读面板标题旁的「推荐」按钮打开模态弹窗：可填写推荐语（可选，最多 500 字）后提交；
  已推荐的文章按钮显示「已推荐」，再次打开可以修改推荐语或「取消推荐」。
- 推荐先写本地库并进入离线队列，界面立即生效；随后自动补一次同步，网络失败时队列保留。
- 「我的推荐」列出全部未撤回的推荐，按推荐时间倒序，显示文章标题、博客、日期与推荐语；
  点击条目打开文章，右侧「取消」也可以撤回推荐。
- 推荐和撤回都需要 `write` scope；只读令牌会保留弹窗并给出明确错误，不会假装成功。
- 真实的推荐行由 `/v1/sync` 下发；网页或其他设备上的改动会在下一次同步后出现在列表里。

### 收藏网页（Shift+B）

- 任意界面按 `Shift+B` 打开「收藏网页」弹窗：输入网址（可直接粘贴不带 `https://` 的地址，
  弹窗打开时若剪贴板里是网址会自动预填），回车或点「收藏」开始下载。
- 下载在后台线程完成，产物是 **Markdown + 图片**：
  1. 用 `dom_smoothie`（Readability.js 的 Rust 移植）抽正文、标题、站点与语言；
  2. 正文经过本地清洗（隐藏节点、懒加载图片、白名单标签、绝对地址），再用 `htmd`
     （turndown.js 的 Rust 移植）转成 Markdown；
  3. 正文里的图片按原始绝对地址下载进 `cached_images`，Markdown 里保留图片链接；
  4. 正文 Markdown 写入 SQLite 的 `documents` 表，之后离线浏览：阅读面板用
     `pulldown-cmark` 把 Markdown 渲染回块 HTML，图片从本地缓存取。
- 「我的收藏」按收藏时间倒序列出标题、站点、日期、图片数与网址（列表右上角也有「＋ 收藏」入口）；
  点击在右侧阅读，行内「移除」删除收藏（连同它的 Markdown 正文）。
  重复收藏同一个网址只保留一条并刷新内容。
- 收藏是**本地资料**，不参与 `/v1/sync`，退出登录或切换账号不会清空；没有登录态也能用。
- 弹窗里会显示下载进度文案与失败原因（地址不合法、页面不是 HTML、抓取超时等），
  失败时弹窗保留，改完地址可直接重试。正文抽取是启发式的：视频页、需要登录的页面
  可能抽不到正文，会给出「没有抽取到可读正文」的提示。
- **按站点适配抓取方式**：微信公众平台（`mp.weixin.qq.com`）用桌面 UA 会被重定向到
  「环境异常 / 完成验证后即可继续访问」验证页，因此公众号文章改用微信内浏览器的 UA
  请求，并只取文章正文容器 `#js_content`；如果站点仍然返回验证页/登录墙，会明确报错
  「这个页面要求人工验证或登录」而**不会把验证页当成文章保存**。
- 正文图片同时识别 `data-src` / `data-original` / `data-lazy-src` 等懒加载属性
  （微信、知乎等站点的正文图片走这些属性）；`display:none` / `hidden` 的不可见节点整棵跳过
  （微信小标题两侧的 `unset` 标记就是这样藏起来的，不会再出现在正文里）。
- **X（Twitter）帖子与长文章**按站点结构抽取：从页面 SSR 数据里取长推文全文
  （`note_tweet`）或文章正文（`article_results.result.content_state` 的 Draft.js 块），
  帖子标题用正文第一行（过长截断），文章标题用文章自己的标题；正文保留段落、小标题、
  列表、链接、粗体等样式与正文图片，引用帖作为引用块附在后面。不会混入
  「Post / Log in / Sign up」这类界面文字，也不会把「作者 on X」当标题。
  x.com 的帖子地址（`/<user>/status/<id>`）和文章地址（`/<user>/article/<id>`）都支持；
  `t.co` 文章短链对匿名客户端会跳到登录页，客户端会提示改用文章页或帖子页地址。
- 收藏的网页没有译文，阅读面板只显示原文，不出现 `原文 | 对照 | 译文` 切换；
  标题旁的 `↗` 仍然打开原网址。

### 离线缓存与加载策略

- **本地优先**：正文、译文和图片第一次成功取回后写入 SQLite；之后打开同一篇时，
  命中缓存的正文、译文和图片直接读本地，不再调用 `/v1/bodies`、`/v1/translations` 或 `/img/…`。
- 每个博客名右侧有「缓存」按钮：点一次在后台缓存该订阅所有未缓存的原文、译文和图片。
  按钮会显示 `3/12`、`已缓存`、`待续 N`（译文还在生成）或 `重试`（失败/图片未完成）；
  缓存任务在后台执行、不阻塞阅读，点击不会顺带切换订阅，开始/完成/失败提示显示在侧栏顶部。
- 批量缓存会跳过已完整的文章；即使文章正文/译文已缓存，也会继续补齐缺失图片。
- **断网自动识别**：网络层请求失败（`TelaError::Net`）后标记离线，阅读面板暂停译文轮询，
  不再反复请求 `/v1/translations/...`；等待译文期间约每 10 秒只做一次轻量 `GET /meta`
  探活，恢复后自动继续。
- 离线时已缓存的正文、译文和图片照常可读；未缓存的图片会在下次联网打开文章或再次
  点「缓存」时补齐。离线时点「缓存」会先探活，仍不通则快速给出失败提示，不逐篇等超时。
- 缓存没有与当前账号绑定：退出登录或切换账号不会删除正文/译文/图片缓存。

### 账号与登录

- 真实模式未登录时，侧栏底部显示「登录 Tela」；点击后打开浏览器完成 OAuth 授权。
- 等待授权时显示授权地址，可「打开授权页 / 复制链接 / 取消」；授权失败可「重试登录」。
- 已登录显示 `@handle` 与 scope，并显示本地库路径，可「退出」。
- GUI 在只读（`read` scope）令牌下不会尝试标记已读或写操作；CLI 的写命令需要
  `write` scope，重新授权后恢复。
- 登录成功后自动同步并刷新列表；启动时已登录也会先同步一轮（网络失败则继续读本地库）。
- 凭据库不可用（Keychain / Secret Service 未解锁等）会在侧栏显示原因，不影响读本地缓存。
- 缺少 `TELA_CLIENT_ID` 时侧栏会提示，并跳过启动同步；设置后重启即可登录。

### 快捷键与鼠标

- 全局：`j` / `k` 下一篇 / 上一篇；`Shift+J` / `Shift+K` 下一个 / 上一个博客；
  `Shift+B` 收藏一个网址（打开「收藏网页」弹窗）。
- 阅读：`o` 原文、`c` 对照、`t` 译文、`↑` / `↓` 逐行滚动、
  `PgUp` / `PgDn` 按视口滚动（一篇到底、不切页）、`r` 重试。
- 鼠标：点击订阅/文章切换，滚轮滚动，正文文字可选中。

### 演示模式

- `--demo` 或 `TELA_DEMO=1` 使用进程内 mock，不需要账号与网络。
- 空库时自动写入 2 个订阅、4 篇文章（正常 / 长章节 / 无译文 / 已读），覆盖三视图、
  长文连续滚动、无译文回退与已读状态；`TELA_DEV_SEED=0` 可关闭演示数据写入。
- 演示数据还包含 1 条推荐，可直接查看「我的推荐」列表；演示模式没有账号，推荐/取消推荐
  会提示需要登录（写操作不会落库）。
- 演示数据不含收藏（收藏是用户自己提交的网址）；按 `Shift+B` 收藏一个真实网址仍然可用
  （需要联网），收藏结果落在同一个本地库。

### 当前限制

- 界面固定深色主题，没有字号/行宽/主题设置页。
- 只有「按订阅」文章列表，没有跨订阅的 Unread / Today / Liked 视图。
- 没有周期性后台同步；目前是启动时同步一次，登录/标记已读等操作后再同步。
- 没有搜索、Discover、文件夹、高亮/笔记、OPML 与自动更新入口。
- 推荐只在「我的推荐」里管理；没有成员个人页或公开推荐页。
- 收藏的正文抽取是启发式的，不解析 `srcset` / `<picture>`，也不执行页面脚本；
  X 帖子只保存当前这一条（含引用帖），不展开整个讨论串；
- Markdown 已经作为收藏正文的规范格式存进本地库，但界面暂时没有「导出 Markdown」入口；
  PDF/DOCX/PPTX 等非 HTML 文件暂不支持（下一阶段再接 `markitdown-core`）；
  需要登录、纯前端渲染或强制人机验证的页面可能抽不到正文（会明确报错，不保存空文章）。
  收藏没有云端同步与导入/导出。
- 缓存没有容量上限、占用统计或清理入口；目前只能手动删除本地库文件。
- 图片只解析 `<img src>`，不处理 `srcset` / `<picture>`。

```bash
# GUI 测试（无头；不需要 DISPLAY/WAYLAND_DISPLAY）
env -u DISPLAY -u WAYLAND_DISPLAY \
  cargo test -p tela-ui --features gui-test --locked

# 无窗口数据链路冒烟（CI 用）
TELA_DB=target/ci-gui.db TELA_DEV_SEED=1 \
  cargo run -p tela-app --features gui --bin tela-gui --locked -- --demo --check
# → tela-gui check: mode=demo feeds=2 articles=4 unread=3 recommendations=1 bookmarks=0
```

![S3 三栏 + 对照阅读](docs/images/gui-s3-reader.png)

![S8 我的收藏 + 收藏的网页阅读](docs/images/gui-s8-bookmarks.png)

![S8 Shift+B 收藏网页弹窗](docs/images/gui-s8-bookmark-dialog.png)

更多截图与逐条验收命令见 [`docs/gui_acceptance.md`](./docs/gui_acceptance.md)。

## 本地缓存与离线数据

SQLite 采用 schema v6（旧库打开时只补表，不丢已有数据）：

| 表 | 内容 |
|---|---|
| `meta` | schema 版本、同步游标、语言、行类型、账号 handle |
| `sync_rows` | 订阅/文章/标题/状态等同步行，按 `(entity, row_key)` 存 wire JSON |
| `pending_mutations` | 离线写队列：`mid` + `at` + mutation JSON |
| `bodies` | `contentKey` → 不可变正文对象 JSON |
| `cached_translations` | `(contentKey, lang)` → 译文状态 + 可读译本对象 JSON（含 pending/none 快照） |
| `cached_images` | 图片 URL → 原始字节 BLOB |
| `bookmarks` | `Shift+B` 收藏的网页：URL、标题、站点、摘要、正文 key、图片数、收藏时间 |
| `documents` | 收藏网页的 Markdown 正文（`content_key` → markdown + 语言 + 更新时间） |

- 打开文章时先读本地；没有正文才请求 `/v1/bodies`，没有译文快照才查翻译状态。
- 收藏的网页正文以 Markdown 放 `documents`（key 以 `web-` 开头），图片放 `cached_images`；
  阅读时 Markdown 渲染成块 HTML，图片从本地缓存读取；这类正文没有远端译文，
  阅读面板直接按原文显示，不查翻译状态。旧版本收藏的块正文存在 `bodies`，仍可读取。
- 没有译文时按需 `POST /v1/translations` 并在后台轮询；完成后写入缓存。
- 图片通过 `ContentApi::load_image` 带认证下载，字节写入 `cached_images`；阅读面板只在内存里
  缓存解码后的 `Image`，跨进程靠 SQLite。
- 退出登录 / 切换账号会清同步行与待发队列，但保留 `bodies` / `cached_translations` /
  `cached_images` 与 `bookmarks`（内容不含账号私有数据）；`sync --full` 的快照 reset 同样不清这些缓存。
- 当前没有缓存容量上限、占用统计或清理命令；需要时可直接删除 `TELA_DB` 指向的库文件。

更细的字段与迁移说明见 [`docs/real-api.md`](./docs/real-api.md)。

## 架构

依赖方向单向：`tela-app → {tela-ui, tela-sync} → {tela-store, tela-content, tela-net} → tela-proto`。

| 路径 | 职责 |
|---|---|
| `crates/tela-proto` | v1 wire 类型（14 张同步表、17 种 mutation、me/usage/meta）、错误、已读规则 |
| `crates/tela-store` | SQLite schema v6：行、不透明游标、离线队列、正文/译文/图片缓存、网页收藏与 Markdown 正文 |
| `crates/tela-sync` | 拉取编排（reset/more/重试）、推送队列、状态快照 |
| `crates/tela-net` | OAuth PKCE、回环回调、系统凭据库、Bearer 客户端、错误映射 |
| `crates/tela-content` | 正文/译本模型、`/v1/bodies`、翻译状态与对象、图片字节接口、网页抓取与 Markdown 抽取（`webpage`/`markdown`） |
| `crates/tela-ui` | 纯视图模型（`shell`/`reader/*`）+ GPUI 视图（`gui/*`） |
| `crates/tela-app` | 装配（`services.rs`）、本地缓存层（`content_cache.rs`）、终端 CLI、GPUI 入口 |
| `tests/fixtures/api/` | 官方 v1 示例响应体（同步/变更/me/usage/meta/翻译） |
| `tests/fixtures/content/` | 正文与译本对象 fixture（含图片 URL） |

### 关键语义

- **cursor 不透明**：与行同事务持久化；换语言/换行类型时客户端主动从快照重来。
- **离线队列**：`pending_mutations` 跨进程保存 `mid` 与 `at`；失败原样重发，有答复后整批丢弃。
- **已读**：按文档的 5 条优先级判定（`readAt` → `markedUnread` → 水位线 → 30 天 → 未读）。
- **令牌**：访问令牌只在内存；刷新令牌只进 Keychain / Credential Manager / Secret Service。
- **正文**：不可变对象，按 `contentKey` 缓存到 `bodies` 表，可离线重读。
- **译文与图片**：分别按 `(contentKey, lang)` 和图片 URL 缓存；首次加载后离线可读，
  批量缓存按钮会连同译文与图片一起预取。
- **离线判定**：请求失败即标记离线；离线时暂停译文轮询，只做低频 `/meta` 探活，
  恢复后自动继续，不需要手动开关。

更完整的接口映射、错误码与安全说明见 [`docs/real-api.md`](./docs/real-api.md)。

## 发布

发布由 [`release.yml`](./.github/workflows/release.yml) 自动完成：

```bash
# tag 去掉 v 前缀后必须等于 Cargo.toml 里 tela-app 的版本
# 仓库变量 TELA_CLIENT_ID 必须在构建前配好（发布包内置它）
git tag -a v0.4.4 -m "Tela Desktop v0.4.4"
git push origin v0.4.4
```

推送 tag 后：

1. 在 Linux 上校验 tag/版本并跑完整测试；
2. 在 `ubuntu-24.04`、`windows-latest`、`macos-15`（Apple Silicon）与
   `macos-15-intel`（Intel）四个平台原生构建 `tela-app` 与 `tela-gui`；
3. 生成
   `tela-desktop-<tag>-x86_64-unknown-linux-gnu.tar.gz`、
   `tela-desktop-<tag>-x86_64-pc-windows-msvc.zip`、
   `tela-desktop-<tag>-aarch64-apple-darwin.tar.gz`、
   `tela-desktop-<tag>-x86_64-apple-darwin.tar.gz` 及对应 `.sha256`；
4. 创建 GitHub Release（自动生成 release notes）。

预发布用带连字符的 tag，会自动标记为 prerelease：

```bash
git tag -a v0.4.4-rc.1 -m "Tela Desktop v0.4.4-rc.1"
git push origin v0.4.4-rc.1
```

Linux GUI 依赖 Wayland/Vulkan/字体等系统库；Windows 包是原生 MSVC 构建；
macOS 提供 Apple Silicon 与 Intel 两个原生包，但**未签名、未公证**。首次运行若被
Gatekeeper 拦截，可在解压目录执行：

```bash
xattr -dr com.apple.quarantine .
./tela-gui
```

### Windows 包是「解压即用」的

Windows 的 exe 静态链接 MSVC/C 运行库（见 [`.cargo/config.toml`](./.cargo/config.toml)
里的 `crt-static`），解压后直接双击 `tela-gui.exe` 即可，**不需要**另行安装
Visual C++ Redistributable。`release.yml` 在打包前会扫描两个 exe 的导入表，
一旦重新引入 `VCRUNTIME140.dll` / `MSVCP140.dll` 就让构建失败。

双击进入的是**登录模式**（发布包内置 public client id）：侧栏底部点「登录 Tela」，
浏览器完成授权后令牌进 Windows 凭据管理器，数据落在 `%APPDATA%\tela\tela.db`。
想离线看演示数据时用 `tela-gui.exe --demo`。

> 启动失败时没有控制台输出，原因会写到 `%APPDATA%\tela\tela-gui-error.log`。

> **v0.2.0 的 Windows 包没有静态 CRT**：双击会报
> `VCRUNTIME140.dll was not found`。请直接用 v0.2.1 及之后的包。

## 文档

| 文档 | 内容 |
|---|---|
| [更新日志](./CHANGELOG.md) | 按版本记录桌面端的主要变化 |
| [真实 API 接入](./docs/real-api.md) | OAuth、同步/推送、正文/翻译、退出码、测试与下一步 |
| [剩余工作盘点](./docs/remaining-work.md) | 对照 M0–M5 与 P0/P1 的逐项状态、估算与下一批切片 |
| [API 方案（历史）](./docs/api-proposal.md) | 服务端公开 API 之前的调研与提议；现状以官方 v1 文档为准 |
| [架构设计](./docs/architecture.md) | 进程/线程、crate 划分、SQLite、同步/reducer、渲染管线 |
| [产品设计](./docs/product-design.md) | 目标用户、功能范围、关键界面、路线图 |
| [开发计划](./docs/development_plan.md) | 阶段/切片、接口契约、验收命令 |
| [GUI 验收](./docs/gui_acceptance.md) | S0–S8 的交付、命令实测、截图 |

## 调研基线

- `hutusi/tela` @ `f57fa6f`（2026-10-25）；API 文档实读于 2026-10-28
- `longbridge/gpui-kit` @ `7b14e03`（`gpui-kit 0.8.0`，GPUI snapshot `gpui-pre 0.3.9`）

## 下一步

1. GUI 的周期性同步与列表自动刷新（开窗前已同步一轮，`AppServices::sync_once` 可复用）；
2. Unread / Today / Liked 列表与「全部已读 + 撤销」；
3. 划线、文件夹、关注、OPML 的 UI；
4. Discover / 站点 / 个人页 / 搜索（公共只读，`lang` 必带）；
5. 缓存管理（容量上限、占用统计、清理入口）、阅读设置页（字号/主题）与自动更新。

## 免责声明

本项目是个人/社区项目，与 Tela 官方无隶属关系。名称、图标、内容的使用需取得作者许可。


## 许可证

代码以 **MIT OR Apache-2.0** 双许可发布，见 `LICENSE-MIT` 与 `LICENSE-APACHE`。
Tela 的名称、图标与内容不在此许可范围内，使用需取得作者许可。
