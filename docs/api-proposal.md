# Tela 客户端 API 方案（讨论稿）

> 目的：为 Tela 桌面客户端（Rust + GPUI）讨论一份稳定、可版本化、可公开的客户端 API。
> 阅读对象：Tela 作者、桌面客户端开发者。
> 状态：**提案**，不是官方规范。文中标记「现状」的内容来自对 `hutusi/tela`
> 源码与 `https://telaread.com` 的实测；标记「提议」的内容尚未实现。
>
> 调研基线：
> - 源码：`hutusi/tela` @ `f57fa6f`（2026-10-25）
> - 线上：`https://telaread.com`，实测于 2026-10-25（Asia/Shanghai），站点处于 private beta
> - 站点 README 明确写着 *"Tela is in private testing"*，新账号需要邀请码

---

## 0. TL;DR

Tela 今天**没有公开 API**，但它内部已经有一套质量很高的客户端协议：
`GET /api/v1/sync`（拉取）+ `POST /api/v1/mutations`（推送）+ 不可变对象
（`/o/*`）+ 少量 RPC。Web 前端就是一个"本地优先"的客户端，桌面端完全可以复用同一套语义。

**建议不要新造 API，而是把已有的客户端协议"公开化"**，只补三件事：

1. **认证**：新增设备授权（Device Flow）+ 个人访问令牌（PAT），同时保留 Cookie 会话给 Web。
2. **能力发现**：新增 `GET /api/v1/client`，返回协议版本、特性开关、限额，客户端不用靠猜。
3. **共同约定**：`Authorization: Bearer`、`x-tela-client` 升到 4、Origin 校验对 Bearer 放行、
   统一限流响应头、OpenAPI/JSON Schema 导出。

这样服务端改动很小（一个 token 表 + 一层中间件 + 一个路由），桌面端拿到的是**与 Web 完全同源**
的数据与语义，不会出现"两个客户端、两套真相"。下面第 12 节给出分阶段改动清单。

---

## 1. 文档范围

**包含**

- 桌面客户端读写 Tela 所需的全部 HTTP 接口（同步、对象、订阅、翻译、高亮、发现等）。
- 认证与授权方案，含三种可选路径和推荐路径。
- 协议语义：分页、游标、幂等、冲突、离线、错误恢复。
- 需要与作者确认的产品/商业/安全问题清单。

**不包含**

- Tela 服务端内部实现（D1、队列、lease、抓取管道）。
- 管理后台 `/api/v1/admin/*`、WebSub 回调、客户端错误上报。
- 桌面端 UI 与架构（见 `architecture.md`）与产品设计（见 `product-design.md`）。

**读者约定**

- 「现状」= 我已从代码或线上行为确认。
- 「提议」= 需要作者拍板或实现。
- 所有时间戳均为 **epoch 毫秒**（与 `packages/sync/src/rows.ts` 一致）。
- 所有 JSON 字段均为 **camelCase**。

---

## 2. 现状：Tela 已经有什么

### 2.1 拓扑

```
客户端 ──▶ tela-web（边缘 Worker）
             ├── /api/*  ──▶ tela-api（Hono，钉在新加坡，紧邻 D1）
             ├── /o/*    ──▶ R2 不可变对象（需会话）
             ├── /img/*  ──▶ 图片代理（需会话）
             └── /avatar/* ─▶ 头像（公开）
```

对客户端而言只有一个源：`https://telaread.com`。

### 2.2 关键的"现状"事实

| 事实 | 证据 | 对桌面端的影响 |
|---|---|---|
| 只有会话 Cookie 认证，没有 token | `apps/api/src/auth.ts`：better-auth，`cookiePrefix: 'tela'`，`session.expiresIn = 60 天`，`updateAge = 1 天` | 原生客户端要自己维护 Cookie Jar，或推动服务端加 token |
| 除 `/api/v1/me` 外的成员接口要求 `x-tela-client >= 3` | `MIN_CLIENT = 3`（`packages/sync/src/protocol.ts`）；`/api/v1/me` 在版本检查之前就返回 | 必须带此头，否则 409 `upgrade` |
| 除 `/api/v1/me` 外，成员接口要求 `x-tela-member: <userId>` | `MEMBER_HEADER`，缺失/不符 → 409 `account_changed` | 登录后先调 `/api/v1/me` 拿 userId |
| 写请求必须带 `Origin: https://telaread.com` | `apps/reader/worker/edge.ts`：`WRITES` + `foreign` 判断；实测无 Origin 的 POST 返回 **403** `cross-origin write refused` | 原生客户端必须显式设置该 Origin（服务端只做字符串比较），或服务端对 Bearer 放行 |
| 完全没有 CORS 响应头 | 实测 `OPTIONS /api/v1/sync` 无 `Access-Control-*` | 纯原生 HTTP 客户端不受影响；浏览器第三方前端不可行 |
| 会话 Cookie 不能用于跨站，`SameSite=Lax` | `edge.ts` 注释 | 同上 |
| `/o/*`、`/img/*` 需要会话 | `serveObject` / `serveImage` 调 `session()`；实测未登录返回 **401** `sign in first` | 对象下载要么带 Cookie，要么服务端支持 Bearer |
| 对象是不可变、内容寻址的 | `OBJECT_CACHE = 'private, max-age=31536000, immutable'` | 可以永久缓存、离线预取 |
| 同步是"单一 `seq` 计数器 + 游标"模型 | ADR 0025，`packages/sync` | 桌面端可完整复刻，服务端不保存每设备状态 |
| 推送是幂等 mutation 批（≤50 条） | `MAX_MUTATIONS_PER_PUSH = 50` | 离线队列天然安全，可重放 |
| 站点是邀请制 private beta | 站点首页 / README | 桌面端也要走邀请码，不能绕过 |
| 存在"翻译配额"与"博客 opt-out" | `USER_DAILY_TRANSLATION_TOKENS = 400_000`；`translation_opt_out` | 桌面端要复用同一套配额与拒绝语义 |

### 2.3 实测到的响应（节选）

```http
GET /api/v1/public/front
HTTP/2 200
cache-control: public, max-age=60, s-maxage=300, stale-while-revalidate=240
content-type: application/json

{"counts":{"blogs":170},"week":{"blogs":90,"languages":10,"posts":309},"edition":{...}}
```

```http
GET /api/v1/sync?cursor=0
HTTP/2 401
{"error":"unauthorized"}
```

```http
POST /api/v1/mutations          # 没有 Origin
HTTP/2 403
cross-origin write refused

POST /api/v1/mutations          # 带 Origin: https://telaread.com
HTTP/2 401
{"error":"unauthorized"}
```

```http
GET /o/c/0123456789abcdef0123456789abcdef.json
HTTP/2 401
sign in first
```

**结论**：协议本身已经够用，缺的是「给非浏览器客户端的一条合法入口」。这就是本方案要解决的全部问题。

---

## 3. 目标与设计原则

### 3.1 目标

1. 桌面客户端能完成 Web 端的全部**阅读**能力：订阅、列表、已读、点赞、推荐、高亮、笔记、翻译。
2. 离线可用：断网时照常阅读已缓存内容，恢复后自动同步。
3. 协议**单一真相**：Web 与桌面共用一套语义、一套测试、一套数据形状。
4. 可撤销、可审计、可限流：作者能知道"哪些设备在用"，能一键踢掉。
5. 服务端改动可控：优先复用现有路由，不重写同步层。

### 3.2 非目标（至少第一阶段）

- 不做第三方应用市场/开放平台。
- 不做写入之外的"管理"能力（管理后台不开放给客户端）。
- 不做服务端推送通道（保持轮询；WebSub 只在服务端内部使用）。
- 不做多账号同时在线（客户端可后续支持，协议不必改）。

### 3.3 原则

| 原则 | 含义 |
|---|---|
| **协议即契约** | 已发布的字段/语义不删不改；只做加法；破坏性变更必须升 `protocol` |
| **客户端可降级** | 服务端回滚后，旧客户端不能崩溃：缺字段按缺省处理（现状代码已这样做） |
| **幂等优先** | 所有写操作可重放；网络失败不需要"到底成没成"的猜测 |
| **本地优先** | 读路径不依赖网络；写路径先落本地再后台推送 |
| **最小权限** | token 分 scope；桌面端默认只要 `read write translate` |
| **成本可见** | 翻译是花钱的操作，客户端必须能显示配额与拒绝原因 |

---

## 4. 认证与授权

### 4.1 三种可选路径

#### 方案 A：复用 Cookie 会话（零服务端改动，仅适合原型）

原生客户端实现一个 Cookie Jar：

1. `POST /api/auth/sign-in/email`（或 `email-otp`）→ 保存 `Set-Cookie: tela.session_token=...`
2. 每个请求带该 Cookie + `Origin: https://telaread.com` + `x-tela-client: 3`
3. `/o/*`、`/img/*` 同样带 Cookie

**优点**：今天就能跑通，不用等作者改任何代码。
**缺点**：

- 需要显式设置 `Origin`，语义上很别扭（它本是浏览器概念）；
- Cookie 权限过大（可改密码、解绑登录方式、登出所有设备）；
- 无法按设备命名、无法细粒度撤销、无法分 scope；
- 60 天有效期 + 每天滚动续期，长期驻留的桌面端等于"永久登录"；
- 一旦 Cookie 泄露，攻击面等于整个账号。

> 建议：**只用于内部原型**，并在文档里明确标注"临时手段"。

#### 方案 B：设备授权 + 个人访问令牌（推荐，第一阶段）

采用与 OAuth 2.0 Device Authorization Grant 相同的交互（RFC 8628），但令牌是 Tela 自己的 PAT。

流程：

```
┌────────────┐        ┌──────────────────┐        ┌────────────────────┐
│ 桌面客户端  │        │   tela-api        │        │ 浏览器（已登录）    │
└─────┬──────┘        └────────┬─────────┘        └─────────┬──────────┘
      │ ① POST /api/v1/device/authorization                  │
      │────────────────────────▶│                          │
      │ {deviceCode, userCode, verificationUri}              │
      │◀────────────────────────│                          │
      │                         │                          │
      │ ② 显示 userCode，打开浏览器 ───────────────────────▶│
      │                         │  GET /activate?code=...   │
      │                         │◀─────────────────────────│
      │                         │  ③ 用户点「允许」          │
      │                         │◀─────────────────────────│
      │ ④ POST /api/v1/device/token {deviceCode}             │
      │────────────────────────▶│                          │
      │ {accessToken, scopes}   │                          │
      │◀────────────────────────│                          │
```

令牌特性：

- 格式：`tela_pat_` + 32 字节随机数的 base64url；**服务端只存 SHA-256 哈希**（与邀请码一致的做法）。
- 有效期：默认 365 天；可配置。
- 绑定：`token_id`、`user_id`、`name`（设备名）、`platform`、`scopes`、`created_at`、`last_used_at`、`revoked_at`。
- 撤销：`DELETE /api/v1/tokens/:id`；或设置页「退出该设备」。
- 频率：每次请求更新 `last_used_at` 要克制（例如 5 分钟节流），避免每次请求写 D1。

**与现有账号安全联动（建议）**：

- 修改密码、重置密码、`POST /api/v1/account/sign-out-everywhere` 时，**同时撤销该成员的
  所有 client token**（或至少标记为"需要重新授权"）；
- 设备授权页必须显示：客户端名称、平台、申请 scope、发起时间，并提供"拒绝"；
- `userCode` 短、无歧义（去掉 0/O/1/I），一次性使用，10 分钟过期；同一 IP 的轮询与
  批准都要限流；
- token 记录 `last_ip`/`last_country`（可选），用于异常提示，不用于封锁。

Scope 设计（最小集合，够桌面端用）：

| Scope | 允许 |
|---|---|
| `read` | `GET /api/v1/sync`、`POST /api/v1/sync/titles`、`/o/*`、`/img/*`、`/avatar/*` |
| `write` | `POST /api/v1/mutations`（已读、点赞、推荐、高亮、文件夹、博客卷、偏好、隐私开关、订阅） |
| `translate` | `POST /api/v1/translations`、`GET /api/v1/translations/:key/:lang` |
| `library` | `POST /api/v1/feeds*`（发现/添加/OPML）、`/api/v1/claims*`、`/api/v1/search` |
| `account` | `PUT /api/v1/profile`、`/api/v1/avatar`、`GET /api/v1/account`、`/api/v1/export` |
| `social` | `GET /api/v1/following`、`/api/v1/sites/:id/followed-readers`、`/api/v1/dashboard` |

桌面端默认申请：`read write translate library`（`account` 只在设置页需要时再申请，或直接包含）。

**优点**：页面里完成授权，客户端从不接触密码；可撤销、可命名、可限流；对 Web 零影响。
**缺点**：服务端要加表、加路由、加一层 Bearer 中间件；需要一张 `/activate` 页面。

#### 方案 C：标准 OAuth 2.1（未来开放第三方时）

- Authorization Code + PKCE，重定向到 `http://127.0.0.1:<随机端口>/callback`（RFC 8252，原生 App 标准做法）。
- 或继续用 Device Flow 给 CLI。
- 好处：第三方客户端可接入；坏处：要维护 client 注册、同意页、scope 描述、token 轮换。

> **建议路线：A → B →（只有真要开放第三方时）C。**
> 第一阶段做 B，A 仅作为 B 上线前的内部验证手段。

### 4.2 提议的认证接口

#### 发起设备授权

```http
POST /api/v1/device/authorization
Content-Type: application/json

{
  "client": { "name": "Tela Desktop", "platform": "macOS 15.3", "appVersion": "0.1.0" },
  "scopes": ["read", "write", "translate", "library"]
}
```

```json
200 OK
{
  "deviceCode": "dc_9f2c...",
  "userCode": "K7QM-3XZP",
  "verificationUri": "https://telaread.com/activate",
  "verificationUriComplete": "https://telaread.com/activate?code=K7QM-3XZP",
  "expiresIn": 600,
  "interval": 5
}
```

#### 轮询换取令牌

```http
POST /api/v1/device/token
Content-Type: application/json

{ "deviceCode": "dc_9f2c..." }
```

```json
200 OK
{
  "accessToken": "tela_pat_...",
  "tokenType": "Bearer",
  "expiresIn": 31536000,
  "scopes": ["read", "write", "translate", "library"],
  "tokenId": "tok_01J..."
}
```

错误（HTTP 400，body 里给原因，便于客户端区分"继续等"与"用户拒绝"）：

| `error` | 含义 | 客户端行为 |
|---|---|---|
| `authorization_pending` | 用户还没点允许 | 按 `interval` 继续轮询 |
| `slow_down` | 轮询太快 | `interval += 5` |
| `access_denied` | 用户拒绝 | 停止，提示重新发起 |
| `expired_token` | 设备码过期 | 重新发起 |
| `invalid_grant` | 设备码不存在/已使用 | 重新发起 |

#### 设备管理

```http
GET /api/v1/tokens
Authorization: Bearer tela_pat_...

200 OK
{
  "tokens": [
    {
      "id": "tok_01J...",
      "name": "Tela Desktop",
      "platform": "macOS 15.3",
      "scopes": ["read", "write", "translate", "library"],
      "createdAt": 1792932061298,
      "lastUsedAt": 1793018461298,
      "expiresAt": 1824468061298,
      "current": true
    }
  ]
}
```

```http
DELETE /api/v1/tokens/tok_01J...
Authorization: Bearer tela_pat_...
→ 204 No Content
```

### 4.3 Bearer 与 Cookie 的共存规则（提议）

1. 服务端先看 `Authorization: Bearer`，有则用 token 认证；否则回退到 Cookie 会话。
2. 用 Bearer 认证时：
   - 不要求 `x-tela-member`（token 已经确定成员）；若带了且不一致 → 409 `account_changed`。
   - **不要求 `Origin`**（这不是浏览器请求）。这需要改 `apps/reader/worker/edge.ts` 的写请求判断。
   - scope 不足 → 403 `insufficient_scope`，并在 `WWW-Authenticate` 里给出所需 scope。
3. 用 Cookie 认证时：保持现状（Origin 校验、`x-tela-member` 校验）。
4. `/o/*`、`/img/*`、`/avatar/*` 由 tela-web 提供，需让它的 `session()` 也接受 Bearer：
   - 最小实现：Bearer 时调一次 tela-api 的 `get-session`（有 D1 读，但对象有边缘缓存，命中率高）；
   - 更好：把 token 校验做成**短期签名凭据**（例如 5 分钟 JWT，内含 `user_id` + `scope`），
     对象请求用签名凭据，避免每个对象都读 D1；撤销靠短 TTL 生效。

### 4.4 客户端侧存储

- macOS：Keychain；Windows：Credential Manager（DPAPI）；Linux：Secret Service（GNOME Keyring / KWallet）。
- 绝不写入明文配置文件；日志、崩溃转储、导出文件一律脱敏。
- 退出登录 = 删除本地 token + 清空本地库（或保留为"离线档案"，需用户显式选择）。

---

## 5. 通用约定（提议）

### 5.1 Base URL 与版本

- Base：`https://telaread.com`
- API 版本在路径里：`/api/v1/...`（已存在）。
- **客户端协议版本**独立于 API 路径版本：`x-tela-client: 4`。
- 提议把 `MIN_CLIENT` 从 3 升到 4 的时机放在"Bearer + `/client` 上线后"，且**服务端同时接受 3 和 4**
  一段时间，避免 Web 旧壳被拒。

### 5.2 请求头

| 头 | 必需 | 说明 |
|---|---|---|
| `Authorization: Bearer <token>` | 二选一 | 原生客户端推荐 |
| `Cookie: tela.session_token=...` | 二选一 | Web 现状 |
| `x-tela-client` | 是 | 协议版本，如 `4` |
| `x-tela-member` | Bearer 时可省 | 会话认证时必填，值为 `/api/v1/me` 的 `id` |
| `Content-Type: application/json` | 有 body 时 | OPML 用 `text/x-opml`，头像用图片类型 |
| `User-Agent` | 建议 | `TelaDesktop/0.1.0 (macOS 15.3; +https://github.com/you/tela-desktop)` |
| `Accept-Language` | 可选 | 影响邮件语言，不影响内容 |
| `If-None-Match` | 可选 | 公开接口可支持 ETag（提议） |

### 5.3 响应约定

- 成功：`2xx` + JSON（除对象与 OPML）。
- 失败：`{"error": "<machine_code>"}`；better-auth 的接口除外（`{"message","code"}`）。
- 已知错误码：

| HTTP | `error` | 含义 |
|---|---|---|
| 400 | `invalid_cursor` / `invalid_titles` / `invalid_ids` / `invalid_push` / `invalid_url` | 参数问题 |
| 401 | `unauthorized` | 无会话/令牌无效 |
| 403 | `forbidden` / `insufficient_scope` / `not_owner` | 无权限 |
| 404 | `not_found` | 资源不存在 |
| 409 | `upgrade` | 客户端协议太旧，请升级 |
| 409 | `account_changed` | `x-tela-member` 与会话不符，需清库重来 |
| 409 | `handle_taken` | 昵称被占用 |
| 413 | `too_large` / `opml_too_large` | 请求体超限 |
| 422 | `unreachable` / `not_a_feed` / `feed_too_large` | 订阅源问题 |
| 429 | `rate_limited` | 触发限流 |
| 500 | `internal` | 服务端错误 |
| 503 | `ok:false` | `/api/health` 的 D1 异常 |

### 5.4 限流响应（提议增强）

```http
HTTP/2 429
Retry-After: 42
X-RateLimit-Limit: 30
X-RateLimit-Remaining: 0
X-RateLimit-Reset: 1792935661
```

现状：多数路由只返回 `{"error":"rate_limited"}`，没有 `Retry-After`。
建议统一补上，客户端才能优雅退避而不是盲目重试。

### 5.5 现有动作限额（现状，供客户端预算）

| 动作 | 限额 | 窗口 |
|---|---|---|
| `discover`（发现源） | 30 | 1 小时 |
| `subscribe`（添加订阅） | 120 | 1 小时 |
| `opmlImport` | 5 | 1 小时 |
| `claimStart` / `claimVerify` | 10 / 30 | 1 小时 |
| `translate`（正文翻译） | 30 | 1 小时 |
| `convert`（简繁转换） | 600 | 1 小时 |
| `export` | 10 | 1 小时 |
| `avatarUpload` | 20 | 1 小时 |
| `otpSend`（按邮箱） | 5 | 1 小时 |
| `passwordSignIn`（按邮箱） | 10 | 15 分钟 |

翻译另有**每日 token 额度**：每人 400,000 tokens/天，单篇上限 40,000。

---

## 6. 接口清单

> 以下路径均为现状（除标注「提议」）。除 `/api/v1/public/*` 与 `/api/v1/join` 外，全部需要认证。
>
> 示例里的 `Authorization: Bearer` 是**提议**的写法；在 Bearer 上线前，等价写法是
> `Cookie: tela.session_token=...` + `x-tela-client: 3` + `Origin: https://telaread.com`（写请求）
> + `x-tela-member: <userId>`。下文不再重复这套兼容头。

### 6.0 我是谁

```http
GET /api/v1/me
Authorization: Bearer ...

200 OK
{
  "id": "q7m2x9k4p1v8c3b6n5j0h7g4f2d9s1a8",
  "email": "reader@example.com",
  "profile": { "user_id": "q7m2x9k4p1v8c3b6n5j0h7g4f2d9s1a8", "handle": "reader", "reading_lang": null, "...": "..." }
}
```

注意：`id` 是 better-auth 生成的 32 位随机字符串（**不是** `usr_` 前缀；以实际返回为准），
`x-tela-member` 与各接口里的 `userId` 都用它。

注意：这里的 `profile` 是 tela-api 的内部行（**snake_case**，字段与同步的 `ProfileRow` 不同），
只用于"知道我是谁"；真正的资料请以 `/api/v1/sync` 的 `profile` 行为准。
`/api/v1/me` 是唯一**同时豁免** `x-tela-client` 与 `x-tela-member` 校验的成员接口
（它正是用来得知该填什么），但仍然需要有效会话。

### 6.1 同步：拉取

```http
GET /api/v1/sync?cursor=<int>&titles=<lang>
Authorization: Bearer ...
x-tela-client: 4
x-tela-member: <userId>
```

| 参数 | 说明 |
|---|---|
| `cursor` | 上次响应的 `cursor`；`0` = 全量快照（`reset: true`） |
| `titles` | 阅读语言：`zh-Hans` / `zh-Hant` / `en` / `fr`；缺省或未知 → 返回全部语言且不带 `titlesLang` |

```json
200 OK
{
  "cursor": 812345,
  "more": false,
  "reset": false,
  "titlesLang": "zh-Hans",
  "rows": {
    "profile": [ { "...": "..." } ],
    "prefs": [],
    "subscriptions": [],
    "feeds": [],
    "sites": [],
    "articles": [
      {
        "id": 6570,
        "feedId": 32,
        "url": "http://www.playpcesor.com/2026/10/....html",
        "title": "我用 Codex 把原創文章變成教學影片…",
        "author": "Esor Huang",
        "publishedAt": 1792927274694,
        "fetchedAt": 1792932061298,
        "sortAt": 1792927274694,
        "sourceLang": "zh-Hant",
        "excerpt": "從已經寫好的文章出發…",
        "contentKey": "9f2c0b7a4e1d8c3f5a6b7c8d9e0f1a2b",
        "wordCount": 1830,
        "readingMinutes": 10,
        "extractState": "done",
        "likeCount": 3,
        "recommendCount": 1,
        "seq": 812301
      }
    ],
    "titles": [],
    "states": [],
    "recommendations": [],
    "highlights": [],
    "claims": [],
    "translations": [],
    "follows": [],
    "folders": [],
    "feedFolders": [],
    "blogrolls": [],
    "blogrollBlogs": []
  },
  "tombstones": [ { "entity": "article", "key": "6123", "seq": 812310 } ]
}
```

语义要点：

- **一页最多 1000 行/表**（`PAGE_ROWS`），页尾落在 `seq` 边界上。
- `more: true` → 立刻用新 `cursor` 再拉一次，直到 `false`。
- `cursor: 0` 是快照：客户端**清空**本地表后写入。
- `reset: true` 还可能出现在"游标超前"时（服务端从备份恢复/回滚，`cursor > head`），
  此时客户端也必须清空重来。**不要**只在 `cursor == 0` 时处理 `reset`。
- 新订阅的 feed 会带出它 30 天内的文章（horizon）。
- 删除用 `tombstones`（目前只有 `article`）。
- 同一 `seq` 是提交顺序，SQLite 单写者保证无空洞。

### 6.2 同步：按 id 取标题（切换阅读语言时）

```http
POST /api/v1/sync/titles
Authorization: Bearer ...

{ "lang": "zh-Hans", "ids": [6570, 6571] }
```

```json
200 OK
{ "rows": [ { "articleId": 6570, "lang": "zh-Hans", "title": "…", "excerpt": "…", "status": "done", "seq": 812400 } ] }
```

- `ids` 最多 1000 个；错误 `invalid_titles` / `invalid_ids`。
- 只返回"已订阅博客"或"保留文章"的标题。

### 6.3 同步：推送

```http
POST /api/v1/mutations
Authorization: Bearer ...
Content-Type: application/json

{
  "mutations": [
    { "mid": "m_01J9Z0K3", "at": 1792932061298, "type": "markRead", "articleId": 6570 },
    { "mid": "m_01J9Z0K4", "at": 1792932061301, "type": "setLiked", "articleId": 6570, "liked": true }
  ]
}
```

```json
200 OK
{
  "applied": ["m_01J9Z0K3", "m_01J9Z0K4"],
  "rejected": [ { "mid": "m_01J9Z0K5", "error": "invalid_range" } ],
  "seq": 812410
}
```

约束：

- ≤ **50** 条/次；body ≤ **1 MB**。
- `mid` 8–64 字符，客户端生成；服务端用 `applied_mutations` 去重 → **重放安全**。
- `at` 客户端时间，服务端**夹到服务器时钟**；冲突按"后写胜出"（LWW）。
- 引用不存在的 article/feed 是 **no-op**，不算错误（不会整批失败）。
- 响应里的 `rejected[].error` 取值（现状）：
  `invalid`（schema 不合法）、`invalid_name`、`invalid_text`、`pref_too_large`、`invalid_range`。
- 响应 `seq` 是推送后的最新 seq；客户端的待确认队列要等拉取到达该 seq 才 `settle`。

### 6.4 对象：正文、翻译、分块、批量

| 路径 | 内容 | 说明 |
|---|---|---|
| `GET /o/c/<contentKey>.json` | 正文对象 | `ContentObject`，见 7.3 |
| `GET /o/t/<contentKey>/<lang>/<sha>.json` | 完整翻译 | `TranslationObject` |
| `GET /o/tc/<contentKey>/<lang>/<request>/<n>.json` | 流式分块 | `TranslationChunk` |
| `GET /o/bundle?k=key1,key2,...` | 批量正文 | 最多 **25** 个；返回 `{ "<key>": <object>, ... }` |
| `GET /img/<contentKey>/<i>` | 图片代理 | 只允许对象里 `images[]` 的索引；raster，≤10 MB |
| `GET /avatar/<userId>?v=<n>` | 头像 | 公开；版本号在地址里 |

- 全部要求认证（除 avatar）。
- 响应头：`cache-control: private, max-age=31536000, immutable`（对象）；图片 7 天。
- 404 = 对象不存在；其他 4xx/5xx 应视为"暂时取不到"，可重试。

### 6.5 翻译

请求：

```http
POST /api/v1/translations
Authorization: Bearer ...

{ "articleId": 6570, "lang": "zh-Hans" }
```

```json
200 OK
{
  "status": "requested",
  "translation": {
    "contentKey": "9f2c...",
    "lang": "zh-Hans",
    "state": "requested",
    "chunkKeys": [],
    "objectKey": null,
    "failedLeaves": [],
    "seq": 812420
  }
}
```

`status` 取值：`requested` / `in_progress` / `ready` / `invalid` / `unavailable` / `rate_limited` / `budget_exhausted`。

`translation.state` 取值（同步行与轮询结果）：`requested` / `running` / `done` / `partial` / `failed` / `skipped`。

轮询：

```http
GET /api/v1/translations/<contentKey>/<lang>
→ 200 { "contentKey": "...", "lang": "...", "state": "running", "chunkKeys": ["tc/.../0.json"], ... }
→ 404 { "error": "not_found" }
```

客户端节奏（与 Web 对齐）：

- 首块未到时：每 **400 ms** 轮询，最多 5 秒；
- 之后：每 **1.5 秒**；
- 有 `chunkKeys` 就下载并叠加；
- `ready` 后取 `objectKey` 的完整对象替换分块。

### 6.6 订阅与 OPML

```http
POST /api/v1/feeds/discover     { "url": "https://example.com" }
→ 200 { "feeds": [ { "url": "https://example.com/feed.xml", "title": "Example",
                     "format": "rss", "itemCount": 12, "homeUrl": "https://example.com" } ] }
→ 422 { "error": "unreachable" | "not_a_feed" | "feed_too_large" }
```

```http
POST /api/v1/feeds              { "feedUrl": "https://example.com/feed.xml" }
→ 200 { "feedId": 123, "siteId": 45 }
```

```http
POST /api/v1/feeds/opml         Content-Type: text/x-opml   (原始 XML 字节，≤1 MB)
→ 200 { "feeds": 12, "subscribed": 10, "folders": 3, "filed": 9 }

GET  /api/v1/feeds/opml         → text/x-opml 附件
```

- 订阅本身也是 mutation（`subscribe`/`unsubscribe`），用于"已存在的 feed"；
- `POST /api/v1/feeds` 只用于"加入一个 Tela 还没有的源"（会触发抓取）。

### 6.7 认领博客

```http
POST /api/v1/claims              { "url": "https://myblog.com" }
→ 200 {
    "siteId": 45,
    "homeUrl": "https://myblog.com",
    "status": "unverified",          // unverified | pending | failed | verified | claimed_by_other
    "error": null,
    "reason": null,
    "github": false,
    "proofs": {
      "meta": "<meta name=\"tela-site-verification\" content=\"...\">",
      "relMe": "<a rel=\"me\" href=\"https://telaread.com/@handle\">Tela</a>"
    }
  }

GET  /api/v1/claims/45           → 同上（当前状态与证明方式）
POST /api/v1/claims/45/verify    → 触发一次校验；返回同一 shape，失败时 error/reason 说明原因
```

- 证明方式是"服务端一次性全试"（meta / `rel=me` / GitHub 双向），不需要客户端选择；
- `token` 是 HMAC 派生的，不落库；`proofs` 里的 HTML 直接展示给用户复制；
- `claimed_by_other` 表示已被别人认领，不能重试。

### 6.8 资料、搜索、导出、账号

```http
PUT  /api/v1/profile             { handle?, displayName?, bio? }        → { ok, handle }
PUT  /api/v1/sites/:id/topics    { topics: [...] }
PUT  /api/v1/sites/:id/translation { optOut: true }
GET  /api/v1/search?q=&lang=     → { sites: [...], articles: [...] }
GET  /api/v1/export              → 一份 JSON（"Your data"）
GET  /api/v1/account             → { email, hasPassword, linked, fresh }
POST /api/v1/account/password    { currentPassword?, newPassword }
POST /api/v1/account/sign-out-everywhere
```

### 6.9 社交（可选）

```http
GET /api/v1/following?tab=all|recs|likes&lang=&cursor=&tz=
GET /api/v1/sites/:siteId/followed-readers
GET /api/v1/dashboard
```

### 6.10 公开接口（无需认证）

| 路径 | 用途 |
|---|---|
| `GET /api/v1/public/auth` | `{ google, github }` 登录方式开关 |
| `GET /api/v1/public/front` | 首页"本期"：每 blog 最新一篇 + 计数 |
| `GET /api/v1/public/discover?topic=&lang=&page=` | Blogs 列表（60/页） |
| `GET /api/v1/public/discover/week` | This week |
| `GET /api/v1/public/discover/articles?topic=&lang=&cursor=` | Articles（30/页） |
| `GET /api/v1/public/discover/readers` | Readers |
| `GET /api/v1/public/discover/blogrolls?cursor=` | 公开博客卷（30/页） |
| `GET /api/v1/public/sites/:id` | 单个博客页 |
| `GET /api/v1/public/profiles/:handle` | 公开资料页 |
| `GET /api/v1/public/blogrolls/:id` | 公开博客卷页 |
| `GET /api/v1/public/handles/:handle` | 昵称是否可用 |

缓存：`public, max-age=60, s-maxage=300, stale-while-revalidate=...`；客户端可据此做本地缓存。

### 6.11 提议新增：能力发现

```http
GET /api/v1/client
```

```json
200 OK
{
  "protocol": 4,
  "minClient": 4,
  "serverTime": 1792932061298,
  "languages": ["zh-Hans", "zh-Hant", "en", "fr"],
  "features": {
    "sync": true,
    "titles": true,
    "bundle": 25,
    "translations": "streamed",
    "claims": true,
    "blogrolls": true,
    "follows": true,
    "deviceFlow": true,
    "bearerObjects": true
  },
  "limits": {
    "pushMutations": 50,
    "pushBytes": 1048576,
    "bodyBytes": 65536,
    "pageRows": 1000,
    "titleIds": 1000,
    "horizonDays": 30,
    "folders": 100,
    "blogrolls": 50,
    "blogrollBlogs": 200,
    "translationTokensPerDay": 400000
  }
}
```

好处：客户端不需要把常量写死；服务端调整限额时旧客户端能自适应；也方便做灰度。

### 6.12 现状没有的能力（客户端不要假设）

| 能力 | 现状 | 桌面端的替代 |
|---|---|---|
| 服务端推送（WebSocket/SSE） | 没有；WebSub 只在服务端内部使用 | 轮询（60s；等待正文/抓取时加速） |
| 正文全文搜索 API | 只按标题/摘要搜 `/api/v1/search` | 本地 FTS + 服务端标题/摘要 |
| 批量拉取指定文章 | 没有（`/sync` 是全量增量） | 全量快照 + 本地筛选 |
| 离线 manifest / 差量对象列表 | 没有 | 用 `/o/bundle` + 本地索引 |
| 客户端设置云端同步 | 有（`prefs` 走 mutation） | 直接用 |
| 推荐流 API | 有 `/following`（关注的人）与公开 Discover | 直接用 |
| 分享/导出卡片 | 没有 | 本地渲染 |
| 客户端错误上报 | 有 `/api/client-errors`，但是浏览器语义（无会话、按 IP 计数） | 先本地日志；是否上报待确认 |

---

## 7. 数据模型

### 7.1 同步行（17 张表）

> 完整定义见 `packages/sync/src/rows.ts`。以下只列桌面端实现必须注意的点。

| 表 | 主键 | 关键字段与语义 |
|---|---|---|
| `profile` | 单行 | `handle`、`readingsLang`、`publicSubscriptions`、`publicLikes`、`avatar`、`isAdmin`；带 `*At` / `*Version` 用于冲突判定 |
| `prefs` | `key` | `value` 是任意小 JSON（≤2 KB）；键名 `[a-z][a-z0-9_.]{0,39}` |
| `subscriptions` | `feedId` | `watermarkId`：id ≤ 它的文章都算已读；`deletedAt` 非空 = 取消订阅 |
| `feeds` | `id` | `status`、`lastFetchedAt`、`lastItemAt`、`mergedInto`（合并源） |
| `sites` | `id` | `homeUrl`、`listing`、`owned`、`claimed`、`readerCount`（<3 时为 0）、`translationOptOut` |
| `articles` | `id` | `sortAt` 排序、`fetchedAt`（30 天视野）、`contentKey`、`sourceLang`、计数 |
| `titles` | `(articleId, lang)` | 标题/摘要的翻译；`status` = `done`/`echo`/`failed` |
| `states` | `articleId` | `readAt`、`readUpdatedAt`（手动未读）、`likedAt`、`likedUpdatedAt` |
| `recommendations` | `articleId` | `note`、`deletedAt` |
| `highlights` | `id` | 锚点：`leafId` + `start`/`end` + `quote`/`prefix`/`suffix`；`side` 是原文/译文 |
| `claims` | `id` | `method`、`token`、`status`、`error` |
| `translations` | `(contentKey, lang)` | `state`、`chunkKeys`、`objectKey`、`failedLeaves` |
| `follows` | `userId` | `handle`、`displayName`、`avatar`、`deletedAt` |
| `folders` | `id`（客户端生成） | `name`、`deletedAt` |
| `feedFolders` | `feedId` | `folderId`（null = 未归档）、`updatedAt`（移动时钟） |
| `blogrolls` | `id`（12 字符） | `name`、`description`、`isPublic`、`publicVersion` |
| `blogrollBlogs` | `(blogrollId, feedId)` | `note`、`noteAt`、`addedAt`、`removedAt`（保留行以便恢复笔记） |

**已知的 `prefs` 键（现状，Web 与桌面共用）**：

| 键 | 值 | 含义 |
|---|---|---|
| `ui.theme` | `system` / `light` / `dark` | 主题 |
| `reader.size` | `s` / `m` / `l` / `xl` | 字号档位（×0.88 / 1 / 1.13 / 1.27） |
| `reader.measure` | `narrow` / `normal` / `wide` | 行宽（560 / 640 / 760 px） |
| `reader.mode` | `side` / `trans` / `orig` | 默认阅读模式 |
| `reader.mark_on_open` | `bool` | 打开即已读 |
| `reader.hide_read` | `bool` | 列表只显示未读 |
| `translate.auto` | `bool` | 打开外文文章自动请求翻译 |
| `translate.never` | `string[]` | 从不翻译的语言标签（如 `["en"]`） |

客户端应**保留不认识的 prefs 键**（同步行是整行下发的），不要把整个表重写掉。

**已读判定（必须与 Web 一致）**：

```
read = state.readAt != null
     || (state.readAt == null && state.readUpdatedAt != null ? false : articleId <= subscription.watermarkId)
     || now > article.fetchedAt + 30 天
```

即"手动标未读"优先于水位线与 30 天视野，直到下一次阅读。

**保留规则**：取消订阅后，文章若被点赞/推荐/有高亮，仍保留；否则删除。

**枚举值（现状）**：

| 字段 | 取值 |
|---|---|
| `sites.listing` | `private` / `listed` / `featured` / `rejected` |
| `articles.extractState` | `none` / `due` / `done` / `failed` |
| `titles.status` | `done` / `echo` / `failed` |
| `translations.state` | `requested` / `running` / `done` / `partial` / `failed` / `skipped` |
| `feeds.status` | `active` / `paused` / `dead` |

### 7.2 变更（25 种）

| 类别 | mutation |
|---|---|
| 阅读 | `markRead`、`markUnread`、`setLiked`、`markAllRead` |
| 订阅 | `subscribe`、`unsubscribe`（可选 `exact`） |
| 偏好 | `setPref`（可选 `ifAbsent`）、`setProfile`、`setPrivacy`（带 `base` 版本） |
| 社交 | `recommend`、`unrecommend`、`follow`、`unfollow` |
| 高亮 | `putHighlight`、`deleteHighlight` |
| 文件夹 | `putFolder`、`deleteFolder`、`fileFeed` |
| 博客卷 | `putBlogroll`、`setBlogrollPublic`、`deleteBlogroll`、`rollBlog`、`unrollBlog`、`noteBlog` |
| 头像 | `setAvatar` |

语义细节：

- `markAllRead` 是"读数水位"：`upTo` 是**客户端实际显示过的最大 id**，避免把刚到的文章一并标已读。
- `setPrivacy` / `setBlogrollPublic` 是**版本化**的：显示（公开）必须带上 `base` 版本，防止旧设备的"公开"覆盖新设备的"隐藏"。
- 删除类 mutation **删除胜出**：高亮、文件夹、博客卷删除后不会被更晚的编辑复活。
- `setLiked` 是绝对值（不是 toggle），两台设备才可能一致。
- `subscribe.exact`：只订阅指定 feed，不跟随 `mergedInto`（用于公开页面上的"全部订阅"）。

### 7.3 对象格式

**正文 `c/<key>.json`**

```json
{
  "format": 1,
  "norm": 7,
  "key": "9f2c0b7a4e1d8c3f5a6b7c8d9e0f1a2b",
  "lang": "zh-Hant",
  "blocks": [
    {
      "tag": "p",
      "html": "<p data-tb=\"a1b2c3d4e5\">我用 Codex 把原創文章…</p>",
      "leaves": ["a1b2c3d4e5"]
    },
    {
      "tag": "figure",
      "html": "<figure><img src=\"/img/9f2c.../0\" data-origin=\"example.com\"></figure>",
      "leaves": []
    }
  ],
  "leaves": { "a1b2c3d4e5": { "hash": "…", "chars": 24 } },
  "images": ["https://example.com/pic.jpg"],
  "stats": { "chars": 1830, "words": 320, "minutes": 10 }
}
```

要点：

- `blocks` 是**顶层块**，顺序即阅读顺序；双语对照按索引对齐。
- 每个带文字的叶子元素有 `data-tb` id；高亮/翻译都按 leaf 定位。
- 图片已被改写成 `/img/<key>/<i>`，**相对路径**；客户端需拼上 `https://telaread.com`。
- `leaves[].skip` 表示该叶子不翻译（代码块等）。

**完整翻译 `t/<key>/<lang>/<sha>.json`**

```json
{
  "format": 1,
  "lang": "zh-Hans",
  "status": "done",
  "blocks": ["<p data-tb=\"...\">…</p>", "…"],
  "failedLeaves": []
}
```

`blocks` 与正文 `blocks` **按索引一一对应**。对不上时（对象损坏/版本错位），客户端应退化为"整篇拼接"而不是错位对齐。

**分块翻译 `tc/<key>/<lang>/<request>/<n>.json`**

```json
{ "format": 1, "n": 0, "blocks": { "0": "<p>…</p>", "1": "<p>…</p>" } }
```

`blocks` 的键是**正文块索引**（字符串），值为译文 HTML。

---

## 8. 协议语义

### 8.1 拉取时序

```
启动 ──▶ 读本地库（立即渲染）──▶ 后台 pull(cursor)
                                   │
                    more=true ─────┤ 立即再拉
                                   ▼
                              applyPull → 渲染增量
```

- 触发点：启动、窗口获得焦点、每 60 秒、每次 push 之后、以及"等待某篇文章正文/某个新 feed 首次抓取"时加速拉取。
- 加速拉取：4 秒起，指数退避到 60 秒，最多持续 2 分钟；窗口不可见时不拉。
- 单次请求超时 30 秒（含读完 body）。

### 8.2 推送时序

```
本地变更 ──▶ 立即写入 pending（持久化）──▶ 250ms 防抖 ──▶ push
                                                   │
                                          失败/离线 ──▶ 指数退避（1s→60s）
```

- 窗口隐藏/关闭时立即 flush（不等防抖）。
- 每次最多 50 条；超过则按顺序分批发。
- 收到 `applied` 不等于可以删除 pending：要等 pull 的 `cursor >= seq` 才 `settle`。

### 8.3 视图模型（必须复刻）

```
confirmed（服务端已确认）  ⊕  pending（未确认/未回传）
        ↓                              ↓
        └────────── view() ────────────┘  →  UI 只渲染 view
```

- `applyPull`：把服务端行合并进 confirmed；`reset` 时先清空。
- `applyMutation`：在本地**预测**服务端效果（同一套规则）。
- `settle`：丢弃 `ackedAt <= cursor` 的 pending。
- 这套 reducer 是纯函数，Web 端有约 900 行实现与 1,200 行测试。
  **桌面端应逐条移植，并用同一批 fixture 做一致性测试**（见 12.3）。

### 8.4 冲突与时钟

- 一律 LWW，比较 `at`；服务端把 `at` 夹到自己的时钟。
- 隐私开关与博客卷公开性是**版本化 LWW**：`show` 必须带 `base`，不匹配则拒绝（不是静默覆盖）。
- 删除胜出（高亮/文件夹/博客卷）。
- 博客卷内博客的排序时钟 `updatedAt` 允许比服务器快 1 天（`PLACE_AHEAD_MS`）。

### 8.5 账号切换

现状语义（Web）：同一浏览器多标签共享 Cookie，因此每个成员调用都带 `x-tela-member`；
不一致时服务端返回 409 `account_changed`，客户端清空本地库并以新账号重来。

桌面端建议：

- 单账号为主；本地库目录按 `memberId` 隔离（`store/<memberId>/`）。
- 切换账号 = 打开另一个 store 目录；不共享 cursor、pending、对象缓存索引。
- 对象缓存可以跨账号共享（内容不可变、不含隐私），但高亮/笔记/已读状态绝不能共享。

---

## 9. 错误与恢复矩阵

| 情况 | 客户端动作 |
|---|---|
| 401 `unauthorized` | 清除 token → 重新设备授权；本地库保留（离线可读），但不再推送 |
| 403 `insufficient_scope` | 提示重新授权并申请所需 scope |
| 409 `upgrade` | 提示升级客户端；**不要**重试 |
| 409 `account_changed` | 停止同步，清空该库，重新登录 |
| 422 `not_a_feed` | 在添加流程里显示"这不是一个 feed"，提供"再试一次/换地址" |
| 422 `feed_too_large` | 显示"源太大，Tela 暂时读不了"，不要重试 |
| 429 | 读 `Retry-After`，指数退避；翻译类错误要显示"今日额度用完" |
| 5xx | 指数退避 + 抖动；连续失败进入"离线模式"提示 |
| 网络错误 | 同上；恢复后先 pull 再 push（避免基于过期状态写） |
| 对象 404 | 视为不存在（可能是内容版本被替换）；显示"正文暂不可用" |
| 对象 5xx/超时 | 可重试；不要删除本地已有副本 |

---

## 10. 兼容性与版本演进

### 10.1 客户端协议版本

- `x-tela-client` 是**客户端协议版本**，不是 App 版本。
- 服务端只接受 `>= MIN_CLIENT`；更低 → 409 `upgrade`。
- 现状 `MIN_CLIENT = 3`，`NAMES_MEMBER_SINCE = 2`。
- 提议：新增 Bearer 与 `/client` 后升到 4，并**同时接受 3**，直到 Web 旧壳自然淘汰。

### 10.2 字段演进规则

1. **只加不删**：新字段必须可选，旧客户端忽略。
2. **缺字段有定义**：例如 `profile` 行缺 `avatar` 时保留本地值，而不是当作 null（现状代码已这样处理）。
3. **枚举可扩展**：未知 `status`/`state` 必须按"未知"处理，不能 panic。
4. **破坏性变更**：升 `protocol`，并至少在 `/client` 里提前一个版本公告。

### 10.3 契约导出（强烈建议）

- 从 zod schema 生成 **JSON Schema / OpenAPI 3.1**，放进仓库 `docs/api/`。
- 导出一组**golden fixtures**（pull 快照、delta、push 响应、对象），供 Rust 客户端做契约测试。
- 这就是"两套客户端、一套真相"的工程保障。

---

## 11. 安全、隐私、合规

### 11.1 安全

- 全站 HTTPS；token 只存哈希；服务端日志不记录 token。
- 对象/图片走 `private` 缓存；客户端缓存目录权限 0700。
- Token scope 最小化；桌面端默认不申请 `account`（避免能改密码）。
- 设备列表可见、可单独撤销；"退出所有设备"仍然有效。
- 防重放：mutation 有 `mid`；token 不参与签名，依赖 TLS。

### 11.2 隐私

- 高亮与笔记是**私有**的，永不出现在公开页。
- 已读状态私有；`readerCount` 在少于 3 人时返回 0/null（不泄露"谁在读"）。
- 客户端崩溃上报必须默认关闭或明确告知，且不包含 URL query、正文、邮箱。
- 本地库建议提供"一键清除"，导出功能对齐 `/api/v1/export`。

### 11.3 内容与商业

- 正文来自第三方博客，Tela 只做抓取/翻译/缓存；客户端**不得**再分发正文。
- 尊重 `translation_opt_out`：该博客的正文不翻译，客户端要显示原因而不是空面板。
- 翻译有成本：桌面端必须复用服务端配额，不能绕过；UI 上要显示"今日额度"。
- Private beta：桌面端也必须邀请制；建议作者给桌面端一个专用邀请码通道或白名单。
- 品牌：客户端名称/图标需作者许可；建议命名为 "Tela for Desktop" 或类似，避免混淆官方。

### 11.4 许可（重要）

- `hutusi/tela` 仓库目前**没有 LICENSE 文件**，`package.json` 标 `"private": true`，
  按默认版权即"保留所有权利"。
- 因此桌面端应作为**独立实现**：只依据公开/文档化的 HTTP 契约编程，不复制其源码、
  不整体搬运其 `packages/*`。
- 若要复用设计 token、图标、文案、字体子集，需作者**书面许可**（或项目方给出明确的开源许可）。
- 桌面端自身的许可由你决定（建议 MIT/Apache-2.0），但不得与上游权利冲突。

### 11.5 作者可能关心的滥用面

| 风险 | 缓解 |
|---|---|
| 第三方客户端抓全站内容 | token scope + 限流 + 只允许已订阅内容；对象需会话 |
| 翻译额度被刷 | 复用 `usage_daily` 与 `action_limits`；按 token 维度再加一层 |
| 客户端伪造 User-Agent 冒充官方 | 官方客户端可用签名/公钥，非必需 |
| 大量对象下载打爆 R2 | 边缘缓存已存在；可加每 token 带宽预算 |
| 账号共享 | 设备列表 + 并发会话上限（可选） |

---

## 12. 服务端改动清单（提议，按阶段）

### 阶段 0：不做任何改动也能跑（1 天）

- 客户端用 Cookie Jar + 伪造 `Origin` + `x-tela-client: 3`。
- 仅用于内部验证协议可跑通。
- 明确标注"临时方案，不对外分发"。

### 阶段 1：Bearer 最小可用（约 3–5 天）

1. 新表 `client_tokens`（或复用 `session` 加 `kind` 字段）：
   `id, user_id, name, platform, token_hash, scopes, created_at, last_used_at, expires_at, revoked_at`。
2. 中间件：`/api/v1/*` 先解析 `Authorization: Bearer`，命中则设置 `member` 与 `scopes`。
3. 写请求 Origin 校验：Bearer 时跳过。
4. `/o/*`、`/img/*`：tela-web 的 `session()` 接受 Bearer（可先每次查 D1，后续再优化）。
5. 手动发 token 的管理命令（`bun run admin token <email>`），先不做设备流。
6. 文档：一页 `docs/CLIENT_API.md` + 一份 OpenAPI。

### 阶段 2：设备授权 + 设备管理（约 1 周）

1. `POST /api/v1/device/authorization`、`POST /api/v1/device/token`。
2. `/activate` 页面（登录后确认，显示客户端名/平台/scope）。
3. `GET/DELETE /api/v1/tokens`。
4. 限流：device 端点单独计数；token 维度限流。
5. `GET /api/v1/client` 能力发现。
6. 统一 429 响应头。

### 阶段 3：契约与生态（持续）

1. zod → OpenAPI/JSON Schema 自动导出，CI 校验。
2. golden fixtures 与跨语言契约测试。
3. 弃用策略与变更日志（`CHANGELOG` 增加 "Client API" 段）。
4. 若开放第三方：OAuth 2.1 + PKCE、client 注册、scope 说明页。

### 改动量评估

| 项 | 规模 | 风险 |
|---|---|---|
| token 表 + 中间件 | 小 | 低（不影响 Web） |
| 对象 Bearer | 小–中 | 中（tela-web 的 session 逻辑） |
| 设备流 + activate 页 | 中 | 低 |
| OpenAPI 导出 | 中 | 低（纯增量） |

---

## 13. 需要和作者讨论的问题

**如果只有 15 分钟，先聊这 5 个：**

1. 是否允许桌面客户端？定位是"官方合作"还是"第三方"？（决定认证方案）
2. 能否把现有 sync/mutations 冻结为公共契约，并导出 schema + fixtures？
3. 认证走设备流 + PAT 可以吗？桌面端默认 scope `read/write/translate/library`。
4. 翻译成本怎么算？桌面与 Web 是否共享每日额度？
5. private beta 的邀请码怎么发给桌面用户？

其余问题按下面的分组展开。

### 产品与商业

1. **是否允许桌面客户端？** 是"官方合作客户端"还是"第三方客户端"？这决定认证方案选 B 还是 C。
2. **是否开源桌面端？** 若开源，token 流程、品牌、图标许可要先说清。
3. **private beta 期间如何给桌面用户发邀请？** 专用邀请码？白名单？还是等公测？
4. **桌面端是否包含社交功能**（推荐、关注、作者 dashboard），还是只做"阅读器"？
5. **是否允许离线保存全文？** 缓存上限、是否加密、退出时是否清除。

### 协议与实现

6. 是否同意把现有 `/api/v1/sync` + `/api/v1/mutations` **冻结为公共契约**？
7. `MIN_CLIENT` 何时升到 4？能否同时接受 3 和 4 一段时间？
8. 是否接受 `GET /api/v1/client` 能力发现端点？
9. 对象接口能否接受 Bearer？还是要求客户端继续用 Cookie？
10. 是否统一 429 的 `Retry-After` 与 `X-RateLimit-*`？
11. 是否愿意导出 OpenAPI / JSON Schema / golden fixtures？
12. 翻译配额是否按"人"还是"设备/令牌"计？桌面端与 Web 是否共享额度？

### 运营与安全

13. 每个账号允许几台设备？是否需要"设备上限"？
14. 是否需要客户端崩溃/错误上报端点（现有 `/api/client-errors` 是浏览器语义）？
15. 是否需要对 token 做 IP/地区限制？
16. 桌面端是否需要"检查更新"的服务端元数据（可放 GitHub Releases，不必用 Tela 域名）？

### 待作者确认的开放问题（我的建议）

- **认证**：先 B（设备流 + PAT），保留 C 作为以后开放第三方的路径。
- **对象**：让 tela-web 接受 Bearer，短期可查 D1，长期用 5 分钟签名凭据。
- **契约**：把 `packages/sync` 作为单一真相，导出 schema + fixtures。
- **范围**：桌面 v1 只做"阅读 + 翻译 + 高亮 + 订阅 + 发现"，写操作限制在阅读状态与收藏。

---

## 14. 附录

### 14.1 一个最小的客户端会话

```http
# 1. 设备授权（提议）
POST /api/v1/device/authorization
→ { userCode, verificationUri, deviceCode, interval }

# 2. 用户在浏览器确认后
POST /api/v1/device/token { deviceCode }
→ { accessToken }

# 3. 我是谁
GET /api/v1/me
Authorization: Bearer ...
→ { id, email, profile }

# 4. 全量快照
GET /api/v1/sync?cursor=0&titles=zh-Hans
→ { cursor, reset: true, rows, tombstones }

# 5. 批量取正文（一次 25 篇）
GET /o/bundle?k=aaa,bbb,ccc
→ { "aaa": {...}, "bbb": {...} }

# 6. 打开文章 → 标记已读
POST /api/v1/mutations
{ "mutations": [ { "mid": "...", "at": 1792932061298, "type": "markRead", "articleId": 6570 } ] }

# 7. 增量拉取
GET /api/v1/sync?cursor=812410&titles=zh-Hans
```

### 14.2 术语

| 术语 | 含义 |
|---|---|
| horizon | 30 天视野：超过 30 天未读的文章自动视为已读 |
| watermark | 每个订阅的"已读水位线"：id ≤ 它即已读 |
| seq | 全局单调递增的提交序号，游标的基础 |
| tombstone | 硬删除记录，告诉客户端"这行没了" |
| contentKey | 正文对象的 32 位十六进制内容哈希 |
| leaf | 带 `data-tb` 的文本叶子，翻译与高亮的最小单位 |
| mutation | 幂等的客户端变更，带 `mid` 与 `at` |
| snapshot / delta | `cursor=0` 的全量 / 基于游标的增量 |

### 14.3 参考

- 源码：`docs/ARCHITECTURE.md`、`docs/adr/0022`–`0025`、`0052`
- 协议：`packages/sync/src/{protocol,rows,mutations,client}.ts`
- 路由：`apps/api/src/app.ts` 与 `apps/api/src/routes/*`
- 边缘：`apps/reader/worker/edge.ts`
- 线上：`https://telaread.com`（private beta）
