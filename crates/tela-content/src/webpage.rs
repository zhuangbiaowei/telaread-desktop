//! 网页收藏：抓取任意 URL，抽出标题、正文块与图片，转成阅读器可渲染的正文对象。
//!
//! 契约（`Shift+B` 收藏一个网址时使用）：
//!
//! - 只接受 `http` / `https`；`javascript:`、`file:` 等一律拒绝；
//! - 正文抽取是**纯函数** [`parse_html`]，网络只在 [`fetch`] / [`fetch_bytes`]，
//!   因此解析规则可以在无网络环境（CI）里完整测试；
//! - 块 HTML 只保留白名单行内标签（链接、强调、代码、图片），第三方页面的
//!   脚本、样式、`on*` 事件属性一律丢弃；链接与图片地址都改写成绝对地址；
//! - 生成的正文对象使用 [`WEB_CONTENT_KEY_PREFIX`] 前缀的**本地** key，
//!   与 Tela 的 `contentKey` 不冲突：`CachingContentApi` 据此跳过远端译文查询，
//!   收藏的网页永远只从本地库读。
//!
//! 解析分工：**正文识别**交给 `dom_smoothie`（Readability.js 的 Rust 移植）；
//! **本地清洗**（隐藏节点、懒加载图片、白名单标签、绝对地址）用
//! `html5ever` + `markup5ever_rcdom` 手写遍历——这一步只需要标签名、属性、文本与
//! 后代遍历，不需要选择器引擎。清洗后的 HTML 再由 `htmd` 转成 Markdown。
//!
//! 站点差异：
//!
//! - **微信公众平台**（`mp.weixin.qq.com`）用桌面 UA 会被 302 到验证页
//!   （「环境异常 / 完成验证后即可继续访问」）。这里按站点选 UA：微信文章用
//!   「微信内浏览器」的 UA 抓，正文容器认 `#js_content`；仍然拿不到正文时返回
//!   [`TelaError::Blocked`]（提示用户去浏览器完成验证），**不保存空文章**。
//! - 懒加载图片（`data-src` / `data-original` / `data-lazy-src`）与微信图片都会取
//!   真实地址，`data:` 占位图被跳过。
//! - **X（Twitter）帖子与长文章**是 React 应用：通用抽取只能拿到导航、登录按钮和
//!   「作者 on X」这种标题。SSR 数据里带着帖子全文（`note_tweet`）或文章正文
//!   （`article_results.result.content_state`，Draft.js 块结构），`x_post_from_html`
//!   直接从脚本里的 JSON 取「作者 + 正文」：帖子的标题用正文第一行，正文按空行分段；
//!   长文章用文章标题，正文按块还原段落、小标题、列表、链接、粗体等样式与图片。
//! - `display:none` / `hidden` 的节点整棵跳过：微信小标题两侧的 `unset` 标记就是
//!   这样藏起来的，之前会被当成正文文字。
//! - 正文最终转成 **Markdown**（`dom_smoothie` 抽正文 → 本地清洗成块 → `htmd` 转
//!   Markdown），图片保留原始绝对地址并单独下载缓存；阅读面板再把 Markdown 渲染回
//!   HTML 块显示（见 [`crate::markdown`]）。

use std::collections::HashMap;
use std::io::Read;
use std::rc::Rc;
use std::time::Duration;

use dom_smoothie::Readability;
use html5ever::tendril::TendrilSink as _;
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use tela_proto::{TelaError, WireContext};

use crate::model::ContentBlock;

/// 本地网页正文 key 的前缀（不与 Tela 的 32 位十六进制 contentKey 冲突）。
pub const WEB_CONTENT_KEY_PREFIX: &str = "web-";

/// 单个网页的最大字节数（避免一个坏地址把内存打满）。
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
/// 单张图片的最大字节数。
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
/// 一次收藏最多下载的图片数量（多余图片下次重新收藏时补齐）。
pub const MAX_IMAGES: usize = 60;

/// 抓取网页的超时。
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// DOM 递归深度上限：恶意/病态页面（成千上万层嵌套）不能把进程栈打爆。
const MAX_DOM_DEPTH: usize = 256;
/// 默认 User-Agent：带上项目地址，方便被站点识别（不伪装浏览器）。
const USER_AGENT: &str = concat!(
    "TelaRead/",
    env!("CARGO_PKG_VERSION"),
    " (+https://telaread.com)"
);

/// 微信公众平台的「微信内浏览器」UA。
///
/// 桌面/普通 UA 访问 `mp.weixin.qq.com/s/…` 会被 302 到 `wappoc_appmsgcaptcha`
/// 验证页（页面文字「环境异常 / 完成验证后即可继续访问」）；用微信内 UA 才能拿到
/// 正常的文章 HTML。公众号文章本来就是面向微信内阅读的，这不是伪装成别的客户端去
/// 绕过登录，而是请求它面向的页面形态。
const WEIXIN_USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) \
     AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148 MicroMessenger/8.0.47(0x18002f2f) \
     NetType/WIFI Language/zh_CN";

/// 微信文章的 Referer（部分图片 CDN 会校验）。
const WEIXIN_REFERER: &str = "https://mp.weixin.qq.com/";

/// 验证/拦截页里的常见文字（小写比较；中文按原样匹配）。
const CHALLENGE_MARKERS: &[&str] = &[
    "环境异常",
    "完成验证后即可继续访问",
    "访问过于频繁",
    "请输入验证码",
    "人机验证",
    "安全验证",
    "just a moment...",
    "enable javascript and cookies",
    "checking your browser",
    "attention required! | cloudflare",
    "cf-challenge",
    "captcha-delivery",
];

/// 验证页的真实 URL 线索（重定向后的地址）。
const CHALLENGE_URL_MARKERS: &[&str] = &[
    "wappoc_appmsgcaptcha",
    "weixin110",
    "challenge-platform",
    "/mp/verify",
    "captcha",
];

/// 正文少于这个字符数、同时命中验证页特征时，判定为「抓到的是验证页」。
///
/// 阈值取高一点：真正被拦截的页面通常只有几十个字；反过来，一篇正常文章即使
/// 引用过「环境异常」这类词，正文长度也远超它，不会被误判。
const CHALLENGE_MAX_ARTICLE_CHARS: i64 = 600;

/// 收藏的一篇网页：正文以 Markdown 保存，图片按原始地址另行下载缓存。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebPage {
    /// 最终地址（跟随重定向之后）。
    pub url: String,
    /// 页面标题（Readability → `og:title` → `<title>` → 正文首个标题 → 主机名）。
    pub title: String,
    /// 站点名（Readability → `og:site_name` → 主机名），可空。
    pub site: String,
    /// 页面声明的语言（Readability → `<html lang>`），可能为空。
    pub lang: String,
    /// 摘要：Readability 的摘要，或第一段正文的前若干字符。
    pub excerpt: String,
    /// 正文 Markdown（图片仍是原始绝对地址）。
    pub markdown: String,
    /// 正文里引用到的图片绝对地址（按出现顺序去重）。
    pub images: Vec<String>,
    /// 正文字符数。
    pub chars: i64,
    /// 正文词数（按空白切分）。
    pub words: i64,
}

impl WebPage {
    /// 是否没有可读正文。
    pub fn is_empty(&self) -> bool {
        self.markdown.trim().is_empty()
    }
}

/// 本地正文 key：`web-` + URL 的 64 位 FNV-1a（十六进制）。
///
/// 同一个 URL 永远得到同一个 key，重复收藏只覆盖本地正文，不会堆出多份；
/// 这个 key 只用于本地库，不参与任何网络协议。
pub fn content_key_for_url(url: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{WEB_CONTENT_KEY_PREFIX}{hash:016x}")
}

/// 是否是本地收藏网页的正文 key（没有远端译文）。
pub fn is_local_page_key(key: &str) -> bool {
    key.starts_with(WEB_CONTENT_KEY_PREFIX)
}

/// 规范化用户输入的地址：补 `https://`、去空白，只允许 http/https。
pub fn normalize_url(raw: &str) -> Result<String, TelaError> {
    let trimmed = raw
        .trim()
        .trim_matches(|ch| matches!(ch, '<' | '>' | '"' | '\'' | ' '))
        .trim();
    if trimmed.is_empty() {
        return Err(config("请输入要收藏的网址"));
    }

    let parsed = match reqwest::Url::parse(trimmed) {
        Ok(url) => url,
        // 用户通常直接粘贴 `example.com/post`：没有 scheme 时按 https 补全。
        Err(_) => reqwest::Url::parse(&format!("https://{trimmed}"))
            .map_err(|error| config(format!("网址不合法：{error}")))?,
    };

    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(config(format!(
                "只支持 http/https 网址，`{other}` 不能收藏"
            )));
        }
    }
    if parsed.host_str().unwrap_or("").is_empty() {
        return Err(config("网址缺少主机名"));
    }
    Ok(parsed.to_string())
}

/// 抓取一个网页并抽取正文（阻塞；GUI 放到后台线程调用）。
pub fn fetch(raw_url: &str) -> Result<WebPage, TelaError> {
    let url = normalize_url(raw_url)?;
    let parsed = reqwest::Url::parse(&url).ok();
    let client = web_client(parsed.as_ref())?;
    let mut request = client
        .get(&url)
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8");
    if let Some(referer) = referer_for(parsed.as_ref()) {
        request = request.header(reqwest::header::REFERER, referer);
    }
    let response = request.send().map_err(|error| net_error(&url, error))?;

    let final_url = response.url().to_string();
    let status = response.status();
    if !status.is_success() {
        return Err(TelaError::Api {
            ctx: WireContext::new(&url, "webpage"),
            status: status.as_u16(),
            code: "http_error".to_string(),
            detail: status
                .canonical_reason()
                .unwrap_or("页面无法访问")
                .to_string(),
            retry_after: None,
        });
    }
    if let Some(content_type) = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase)
        && !(content_type.contains("html") || content_type.contains("xml"))
    {
        return Err(config(format!(
            "这个地址返回的不是网页（{content_type}），无法收藏"
        )));
    }

    let html = read_text(response, MAX_PAGE_BYTES)?;

    // X/Twitter 帖子与长文章：通用抽取会拿到导航/登录按钮/作者名，优先按 SSR 结构抽全文。
    let is_x_post = is_x_post_url(&final_url) || is_x_post_url(&url);
    if is_x_post && let Some(page) = x_post_from_html(&final_url, &html) {
        return Ok(page);
    }
    // t.co 短链（以及直接粘贴的 /i/article/<id>）对匿名客户端会被重定向到登录页；
    // 这不是一条可收藏的正文，明确告诉用户改用什么地址，而不是把登录页存成收藏。
    if let Some(detail) = x_login_gate_detail(&final_url) {
        return Err(TelaError::Blocked { url, detail });
    }

    let mut page = parse_html(&final_url, &html);
    if is_x_post {
        // 文章卡的 SSR 有时只有标题/摘要，正文仍来自页面 DOM（通用抽取）。
        if let Some(title) = x_article_title(&html, &final_url) {
            page.title = title;
        }
        page.site = "X".to_string();
    }
    if let Some(detail) = challenge_detail(&final_url, &html, page.chars) {
        return Err(TelaError::Blocked {
            // 给用户看的是他们提交的地址（不是带一次性 token 的验证页地址）。
            url,
            detail,
        });
    }
    Ok(page)
}

/// 判断「抓到的其实是一个人工验证/拦截页」，返回给用户看的说明。
///
/// `article_chars` 是从页面里抽出的正文字符数：只有正文很薄、同时命中验证页特征
/// 时才判定为拦截页，避免把恰好引用了「环境异常」等词的长文误判。
pub fn challenge_detail(final_url: &str, html: &str, article_chars: i64) -> Option<String> {
    if article_chars >= CHALLENGE_MAX_ARTICLE_CHARS {
        return None;
    }
    let lower_url = final_url.to_ascii_lowercase();
    let url_hit = CHALLENGE_URL_MARKERS
        .iter()
        .any(|marker| lower_url.contains(marker));
    let lower_html = html.to_ascii_lowercase();
    let html_hit = CHALLENGE_MARKERS.iter().any(|marker| {
        if marker.is_ascii() {
            lower_html.contains(marker)
        } else {
            html.contains(marker)
        }
    });
    if !url_hit && !html_hit {
        return None;
    }

    let weixin = lower_url.contains("weixin") || html.contains("微信公众平台");
    Some(if weixin {
        "微信公众平台返回的是「环境异常」验证页，没有返回文章内容".to_string()
    } else {
        "这个页面要求人工验证或登录，没有返回正文".to_string()
    })
}

/// 下载一张图片的原始字节（阻塞；失败由调用方忽略或计数）。
pub fn fetch_bytes(raw_url: &str) -> Result<Vec<u8>, TelaError> {
    let url = normalize_url(raw_url)?;
    let parsed = reqwest::Url::parse(&url).ok();
    let client = web_client(parsed.as_ref())?;
    let mut request = client.get(&url);
    if let Some(referer) = referer_for(parsed.as_ref()) {
        request = request.header(reqwest::header::REFERER, referer);
    }
    let response = request.send().map_err(|error| net_error(&url, error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(TelaError::Api {
            ctx: WireContext::new(&url, "image"),
            status: status.as_u16(),
            code: "http_error".to_string(),
            detail: status
                .canonical_reason()
                .unwrap_or("图片无法访问")
                .to_string(),
            retry_after: None,
        });
    }
    // 有些站点把图片错误页当成 200 返回：明确是 HTML 的直接放弃。
    if let Some(content_type) = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase)
        && content_type.contains("text/html")
    {
        return Err(config("图片地址返回的是网页，已跳过"));
    }
    read_bytes(response, MAX_IMAGE_BYTES)
}

/// 把一段 HTML 抽成 [`WebPage`]（纯函数，不做 IO）。
///
/// `url` 用于把相对链接/图片改写成绝对地址，必须是绝对地址。
pub fn parse_html(url: &str, html: &str) -> WebPage {
    let base = reqwest::Url::parse(url).ok();
    let document = parse_document(html);

    // Readability（dom_smoothie）负责通用网页的正文/标题/站点/语言；正文太少或
    // 解析失败时回退到本地启发式容器选择。
    let readable = readability_article(url, html);

    let article_html = if let Some(weixin) = weixin_content_html(&document, base.as_ref()) {
        // 公众号文章：正文在 #js_content，站点特例优先于 Readability。
        weixin
    } else if let Some(article) = readable
        .as_ref()
        .filter(|article| article.text_content.chars().count() >= MIN_READABLE_CHARS)
    {
        article.content.to_string()
    } else {
        fallback_article_html(&document, base.as_ref())
    };

    // 统一清洗成阅读器块（隐藏节点、懒加载图片、绝对地址、白名单标签），
    // 再转 Markdown：站点特例与通用路径共用同一套清洗规则。
    let (blocks, images) = blocks_from_html(&article_html, base.as_ref());
    let article_html = blocks_to_html(&blocks);
    let markdown = html_to_markdown(&article_html, base.as_ref());

    let lang = readable
        .as_ref()
        .and_then(|article| article.lang.clone())
        .map(|lang| collapse_whitespace(&lang))
        .filter(|lang| !lang.is_empty())
        .unwrap_or_else(|| {
            find_first(&document, "html")
                .and_then(|handle| attr(&handle, "lang"))
                .map(|value| value.trim().to_string())
                .unwrap_or_default()
        });
    let title = readable
        .as_ref()
        .map(|article| collapse_whitespace(&article.title))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| {
            let root = content_root(&document);
            clean_social_title(&extract_title(&document, root.as_ref(), base.as_ref()))
        });
    let site = readable
        .as_ref()
        .and_then(|article| article.site_name.clone())
        .map(|site| collapse_whitespace(&site))
        .filter(|site| !site.is_empty())
        .unwrap_or_else(|| extract_site(&document, base.as_ref()));

    let (chars, words, fallback_excerpt) = stats_from_blocks(&blocks);
    let excerpt = readable
        .as_ref()
        .and_then(|article| article.excerpt.clone())
        .map(|excerpt| collapse_whitespace(&excerpt))
        .filter(|excerpt| !excerpt.is_empty())
        .unwrap_or(fallback_excerpt);

    WebPage {
        url: url.to_string(),
        title,
        site,
        lang,
        excerpt,
        markdown,
        images,
        chars,
        words,
    }
}

/// Readability 抽出的正文至少要有这么多字符，才压过本地启发式结果。
///
/// 太短的“正文”通常意味着 Readability 选中了导航/摘要块，此时回退到按正文容器
/// 搜索更可靠。
const MIN_READABLE_CHARS: usize = 200;

/// 用 Readability（dom_smoothie）抽一次正文；失败返回 `None`。
fn readability_article(url: &str, html: &str) -> Option<dom_smoothie::Article> {
    let mut readability = Readability::new(html, Some(url), None).ok()?;
    readability.parse().ok()
}

/// 微信文章的正文容器（`#js_content`），按本地规则清洗成 HTML。
fn weixin_content_html(document: &Handle, base: Option<&reqwest::Url>) -> Option<String> {
    let host = base.and_then(|url| url.host_str()).unwrap_or("");
    if !is_weixin_article_host(host) {
        return None;
    }
    let content = find_by_id(document, "js_content")?;
    let mut blocks = Vec::new();
    let mut images = Vec::new();
    collect_blocks(&content, base, &mut blocks, &mut images, 0);
    Some(blocks_to_html(&blocks))
}

/// 本地启发式：按正文容器抽块并还原成 HTML。
fn fallback_article_html(document: &Handle, base: Option<&reqwest::Url>) -> String {
    let root = content_root(document);
    let mut blocks = Vec::new();
    let mut images = Vec::new();
    if let Some(root) = root.as_ref() {
        collect_blocks(root, base, &mut blocks, &mut images, 0);
    }
    blocks_to_html(&blocks)
}

/// 把一段 HTML（正文片段或整页）清洗成块与图片列表。
pub(crate) fn blocks_from_html(
    html: &str,
    base: Option<&reqwest::Url>,
) -> (Vec<ContentBlock>, Vec<String>) {
    let document = parse_document(html);
    let mut blocks = Vec::new();
    let mut images = Vec::new();
    if let Some(body) = find_first(&document, "body") {
        collect_blocks(&body, base, &mut blocks, &mut images, 0);
    }
    (blocks, images)
}

/// 块 HTML 依次拼接（`<article>` 由调用方按需包裹）。
fn blocks_to_html(blocks: &[ContentBlock]) -> String {
    let mut html = String::new();
    for block in blocks {
        html.push_str(&block.html);
        html.push('\n');
    }
    html
}

/// 块 HTML → Markdown；`htmd` 失败时退回按纯文本段落输出（不让收藏失败）。
fn html_to_markdown(html: &str, base: Option<&reqwest::Url>) -> String {
    let markdown = htmd::convert(html).unwrap_or_else(|_| plain_text_markdown(html));
    resolve_markdown_destinations(&markdown, base)
}

/// 把 Markdown 里的相对链接/图片地址改成绝对地址。
///
/// `htmd` 只做 HTML→Markdown，不改写地址；阅读器需要绝对地址才能下载图片。
fn resolve_markdown_destinations(markdown: &str, base: Option<&reqwest::Url>) -> String {
    let Some(base) = base else {
        return markdown.to_string();
    };
    // 代码块里的 `](...)` 是示例代码，不能改写。
    let mut out = String::with_capacity(markdown.len());
    let mut fenced = false;
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            out.push_str(line);
            continue;
        }
        if fenced {
            out.push_str(line);
        } else {
            out.push_str(&resolve_inline_destinations(line, base));
        }
    }
    out
}

/// 改写一行 Markdown 里的 `](目标)` 地址（跳过代码块由调用方负责）。
fn resolve_inline_destinations(markdown: &str, base: &reqwest::Url) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut cursor = 0usize;
    while let Some(offset) = markdown[cursor..].find("](") {
        let open = cursor + offset + 2;
        let Some((dest_start, dest_end)) = markdown_destination_bounds(markdown, open) else {
            out.push_str(&markdown[cursor..open]);
            cursor = open;
            continue;
        };
        out.push_str(&markdown[cursor..dest_start]);
        let dest = &markdown[dest_start..dest_end];
        match absolute_url(Some(base), dest) {
            Some(absolute) if absolute != dest => out.push_str(&absolute),
            _ => out.push_str(dest),
        }
        cursor = dest_end;
    }
    out.push_str(&markdown[cursor..]);
    out
}

/// 找到 `](` 之后的目标地址范围（`<…>` 或裸地址，支持括号配对）。
fn markdown_destination_bounds(markdown: &str, open: usize) -> Option<(usize, usize)> {
    let bytes = markdown.as_bytes();
    let mut index = open;
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if index >= bytes.len() {
        return None;
    }
    if bytes[index] == b'<' {
        let start = index + 1;
        let end = markdown[start..].find('>')? + start;
        return Some((start, end));
    }
    let start = index;
    let mut depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                index += 2;
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            byte if byte.is_ascii_whitespace() && depth == 0 => break,
            _ => {}
        }
        index += 1;
    }
    if index > start {
        Some((start, index))
    } else {
        None
    }
}

/// 极端情况下的 Markdown 兜底：把 HTML 文本按行拆成段落。
fn plain_text_markdown(html: &str) -> String {
    let text = plain_text(html);
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 解析 HTML 文档（html5ever 的读取错误只可能来自输入流，按空页处理）。
fn parse_document(html: &str) -> Handle {
    html5ever::parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut html.as_bytes())
        .unwrap_or_default()
        .document
}

// ---------------------------------------------------------------------------
// X（Twitter）帖子与长文章
// ---------------------------------------------------------------------------

/// 是否是 X（Twitter）的帖子地址：`/<user>/status/<id>`。
pub fn is_x_status_url(url: &str) -> bool {
    x_path_has_kind(url, "status")
}

/// 是否是 X 的长文章地址：`/<user>/article/<id>`（以及分享短链落到的 `/i/article/<id>`）。
pub fn is_x_article_url(url: &str) -> bool {
    x_path_has_kind(url, "article")
}

/// 是否是 X 的帖子或长文章地址（正文都在 SSR 数据里，走专门的解析）。
pub fn is_x_post_url(url: &str) -> bool {
    is_x_status_url(url) || is_x_article_url(url)
}

/// 路径第二段是否是 `kind`（`/<user>/<kind>/<id>`）。
fn x_path_has_kind(url: &str, kind: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if !is_x_host(parsed.host_str().unwrap_or("")) {
        return false;
    }
    let segments: Vec<&str> = parsed
        .path()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    segments.len() >= 3 && segments[1] == kind && !segments[0].is_empty()
}

/// X / Twitter 的主机名（`www.`、`mobile.` 前缀不算站点差异）。
fn is_x_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let host = host
        .strip_prefix("www.")
        .or_else(|| host.strip_prefix("mobile."))
        .unwrap_or(&host);
    host == "x.com" || host == "twitter.com"
}

/// t.co 短链与 `/i/article/<id>` 对匿名客户端会被重定向到登录页；给出可操作的说明。
///
/// 公开的帖子页（`/<user>/status/<id>`）与文章页（`/<user>/article/<id>`）不需要登录，
/// 只是分享短链落到的内部文章 id 需要会话才能展开成公开地址。
fn x_login_gate_detail(final_url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(final_url).ok()?;
    if !is_x_host(parsed.host_str().unwrap_or("")) {
        return None;
    }
    let path = parsed.path().to_ascii_lowercase();
    if !(path.contains("onboarding") || path.contains("login") || path.contains("flow")) {
        return None;
    }
    let lower = final_url.to_ascii_lowercase();
    if !(lower.contains("%2fi%2farticle%2f") || lower.contains("/i/article/")) {
        return None;
    }
    Some(
        "这个 X 文章短链要求登录后才能展开，请改用文章页（/<user>/article/<id>）\
         或帖子页（/<user>/status/<id>）地址"
            .to_string(),
    )
}

/// 从 X 页面的 SSR 数据里抽出一篇可读的「文章」。
///
/// 长文章正文在 `result.article.article_results.result.content_state`（Draft.js 块结构），
/// 长推文正文在 `note_tweet`。两者都从脚本里的 GraphQL 结果 `tweet_result_by_rest_id`
/// 取；同一页可能出现多份（文章卡只有标题/摘要，完整帖子才有正文），取信息最全的一份。
fn x_post_from_html(url: &str, html: &str) -> Option<WebPage> {
    let root = x_tweet_result(html, url)?;
    // `tweet_result_by_rest_id` 外面还包了一层 `{id, rest_id, result}`。
    let tweet = root.get("result").unwrap_or(&root);
    if let Some(page) = x_article_from_tweet(url, tweet) {
        return Some(page);
    }
    x_post_text_from_tweet(url, tweet)
}

/// 长文章：标题、作者、Draft.js 块（段落/小标题/列表/链接/样式/图片）。
fn x_article_from_tweet(url: &str, tweet: &serde_json::Value) -> Option<WebPage> {
    let article = tweet.pointer("/article/article_results/result")?;
    let content_state = article.get("content_state")?;
    let blocks_json = content_state.get("blocks")?.as_array()?;
    if blocks_json.is_empty() {
        return None;
    }

    let entities = x_entities(content_state);
    let media_urls = x_article_media_urls(article);
    let base = reqwest::Url::parse(url).ok();
    let mut blocks = Vec::new();
    let mut images = Vec::new();

    let author = x_user_field(tweet, "name").unwrap_or_default();
    let screen_name = x_user_field(tweet, "screen_name").unwrap_or_default();
    push_x_author(&mut blocks, &author, &screen_name);

    let mut list: Option<(&'static str, Vec<String>)> = None;
    for block_value in blocks_json {
        let block_type = block_value
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("unstyled");
        let text = block_value
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match block_type {
            // 图片等媒体在 Draft.js 里是 `atomic` 块，真正的地址在 entity_map 里。
            "atomic" => {
                flush_x_list(&mut list, &mut blocks);
                let Some(entity) = x_block_entity(block_value, &entities) else {
                    continue;
                };
                if entity.kind != "MEDIA" {
                    continue;
                }
                for media_id in &entity.media_ids {
                    let Some(src) = media_urls.get(media_id) else {
                        continue;
                    };
                    if !images.iter().any(|item| item == src) {
                        images.push(src.clone());
                    }
                    blocks.push(block(
                        "figure",
                        format!("<p><img src=\"{}\" alt=\"图片\"></p>", escape_html(src)),
                    ));
                }
            }
            "unordered-list-item" | "ordered-list-item" => {
                let tag = if block_type == "ordered-list-item" {
                    "ol"
                } else {
                    "ul"
                };
                if list.as_ref().is_none_or(|(kind, _)| *kind != tag) {
                    flush_x_list(&mut list, &mut blocks);
                    list = Some((tag, Vec::new()));
                }
                let inner = render_x_inline(text, block_value, &entities, base.as_ref());
                let inner = inner.trim();
                if !inner.is_empty()
                    && let Some((_, items)) = list.as_mut()
                {
                    items.push(format!("<li>{inner}</li>"));
                }
            }
            _ => {
                flush_x_list(&mut list, &mut blocks);
                match block_type {
                    "code-block" | "code" => {
                        if !text.trim().is_empty() {
                            blocks.push(block(
                                "pre",
                                format!("<pre><code>{}</code></pre>", escape_html(text)),
                            ));
                        }
                    }
                    _ => {
                        let tag = match block_type {
                            "header-one" => "h1",
                            "header-two" => "h2",
                            "header-three" => "h3",
                            "blockquote" => "blockquote",
                            _ => "p",
                        };
                        let inner = render_x_inline(text, block_value, &entities, base.as_ref());
                        let inner = inner.trim();
                        if !inner.is_empty() {
                            blocks.push(block(tag, format!("<{tag}>{inner}</{tag}>")));
                        }
                    }
                }
            }
        }
    }
    flush_x_list(&mut list, &mut blocks);

    let title = article
        .get("title")
        .and_then(|value| value.as_str())
        .map(collapse_whitespace)
        .filter(|title| !title.is_empty())
        .or_else(|| {
            article
                .get("plain_text")
                .and_then(|value| value.as_str())
                .and_then(smart_title_from_text)
        })
        .unwrap_or_else(|| "X 文章".to_string());

    let html = blocks_to_html(&blocks);
    let markdown = html_to_markdown(&html, base.as_ref());
    let (chars, words, excerpt) = stats_from_blocks(&blocks);
    let lang = tweet
        .pointer("/legacy/lang")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();

    Some(WebPage {
        url: url.to_string(),
        title,
        site: "X".to_string(),
        lang,
        excerpt,
        markdown,
        images,
        chars,
        words,
    })
}

/// 长推文：标题取正文第一行，正文按空行分段，引用帖作为引用块附在后面。
fn x_post_text_from_tweet(url: &str, tweet: &serde_json::Value) -> Option<WebPage> {
    let text = x_tweet_text(tweet)?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    let author = x_user_field(tweet, "name").unwrap_or_default();
    let screen_name = x_user_field(tweet, "screen_name").unwrap_or_default();
    let lang = tweet
        .pointer("/legacy/lang")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();

    let mut blocks = Vec::new();
    push_x_author(&mut blocks, &author, &screen_name);
    for paragraph in split_paragraphs(text) {
        blocks.push(block("p", format!("<p>{}</p>", escape_html(&paragraph))));
    }

    // 引用的推文：作为引用块附在正文后面（标题仍取主贴第一行）。
    if let Some(quoted) = tweet.pointer("/quoted_tweet_results/result")
        && let Some(quoted_text) = x_tweet_text(quoted)
    {
        let quoted_name = x_user_field(quoted, "name").unwrap_or_default();
        let quoted_screen = x_user_field(quoted, "screen_name").unwrap_or_default();
        let label = match (quoted_name.is_empty(), quoted_screen.is_empty()) {
            (false, false) => format!("{quoted_name}（@{quoted_screen}）"),
            (false, true) => quoted_name,
            (true, false) => format!("@{quoted_screen}"),
            (true, true) => "被引用的帖子".to_string(),
        };
        blocks.push(block(
            "blockquote",
            format!("<blockquote><p>{}</p></blockquote>", escape_html(&label)),
        ));
        for paragraph in split_paragraphs(&quoted_text) {
            blocks.push(block(
                "blockquote",
                format!(
                    "<blockquote><p>{}</p></blockquote>",
                    escape_html(&paragraph)
                ),
            ));
        }
    }

    let images = x_media_urls(tweet);
    for src in &images {
        blocks.push(block(
            "figure",
            format!("<p><img src=\"{}\" alt=\"图片\"></p>", escape_html(src)),
        ));
    }

    let html = blocks_to_html(&blocks);
    let markdown = html_to_markdown(&html, None);
    let (chars, words, excerpt) = stats_from_blocks(&blocks);
    let title = smart_title_from_text(text).unwrap_or_else(|| "X 帖子".to_string());

    Some(WebPage {
        url: url.to_string(),
        title,
        site: "X".to_string(),
        lang,
        excerpt,
        markdown,
        images,
        chars,
        words,
    })
}

/// 作者行（正文第一段）。
fn push_x_author(blocks: &mut Vec<ContentBlock>, author: &str, screen_name: &str) {
    if author.is_empty() {
        return;
    }
    let label = if screen_name.is_empty() {
        author.to_string()
    } else {
        format!("{author}（@{screen_name}）")
    };
    blocks.push(block("p", format!("<p>{}</p>", escape_html(&label))));
}

/// 把连续的列表项合并成一个 `<ul>` / `<ol>` 块。
fn flush_x_list(list: &mut Option<(&'static str, Vec<String>)>, blocks: &mut Vec<ContentBlock>) {
    if let Some((tag, items)) = list.take()
        && !items.is_empty()
    {
        blocks.push(block(tag, format!("<{tag}>{}</{tag}>", items.join(""))));
    }
}

/// 取 SSR 脚本里的 `tweet_result_by_rest_id` 对象。
///
/// 同页多份时先按 URL 里的帖子/文章 id 过滤（时间线、回复里可能混进别的文章），
/// 再取信息最全的一份。
fn x_tweet_result(html: &str, url: &str) -> Option<serde_json::Value> {
    let mut results = x_tweet_results(html);
    if let Some(target) = x_post_id(url) {
        let matching: Vec<serde_json::Value> = results
            .iter()
            .filter(|root| x_result_rest_id(root).as_deref() == Some(target.as_str()))
            .cloned()
            .collect();
        if !matching.is_empty() {
            results = matching;
        }
    }
    results.into_iter().max_by_key(x_tweet_score)
}

/// URL 里的帖子/文章 id（`/<user>/status/<id>`、`/<user>/article/<id>`）。
fn x_post_id(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if !is_x_host(parsed.host_str().unwrap_or("")) {
        return None;
    }
    let segments: Vec<&str> = parsed
        .path()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() >= 3 && matches!(segments[1], "status" | "article") {
        Some(segments[2].to_string())
    } else {
        None
    }
}

/// SSR 结果自己的 rest_id（`{id, rest_id, result}` 的外层）。
fn x_result_rest_id(root: &serde_json::Value) -> Option<String> {
    root.get("rest_id")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| {
            root.get("result")
                .and_then(|result| result.get("rest_id"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
}

/// 一份 SSR 结果的可用程度：完整长文章 > 带正文的帖子 > 文章卡/时间线片段。
fn x_tweet_score(root: &serde_json::Value) -> usize {
    let tweet = root.get("result").unwrap_or(root);
    let mut score = 0;
    if tweet
        .pointer("/article/article_results/result/content_state")
        .is_some()
    {
        score += 100;
    }
    if x_tweet_text(tweet).is_some() {
        score += 10;
    }
    if tweet.get("core").is_some() {
        score += 1;
    }
    score
}

/// 扫描页面里所有脚本的 `tweet_result_by_rest_id`（数据在 JS 字符串里）。
fn x_tweet_results(html: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(found) = html[cursor..].find("<script") {
        let script_start = cursor + found;
        let Some(end_found) = html[script_start..].find("</script>") else {
            break;
        };
        let script_end = script_start + end_found;
        let script = &html[script_start..script_end];
        if script.contains("tweet_result_by_rest_id") {
            let unescaped = js_unescape(script);
            // 数据在 JS 字符串里：`value:"{"data":{"tweet_result_by_rest_id":{…}}}"`。
            let key = "\"tweet_result_by_rest_id\":{";
            let mut position = 0;
            while let Some(found) = unescaped[position..].find(key) {
                let key_pos = position + found;
                if let Some(object) = json_object_slice(&unescaped, key_pos + key.len() - 1)
                    && let Ok(value) = serde_json::from_str(object)
                {
                    out.push(value);
                }
                position = key_pos + 1;
            }
        }
        cursor = script_end + "</script>".len();
    }
    out
}

/// 文章卡的标题（状态页的 SSR 里可能只有标题/摘要，没有完整正文）。
fn x_article_title(html: &str, url: &str) -> Option<String> {
    let root = x_tweet_result(html, url)?;
    let tweet = root.get("result").unwrap_or(&root);
    let title = tweet
        .pointer("/article/article_results/result/title")?
        .as_str()?;
    let title = collapse_whitespace(title);
    (!title.is_empty()).then_some(title)
}

/// 长文章的实体表（`entity_map`）：链接地址与媒体 id。
struct XEntity {
    kind: String,
    url: Option<String>,
    media_ids: Vec<String>,
}

/// 把 `content_state.entity_map` 变成「key → 实体」的查找表。
fn x_entities(content_state: &serde_json::Value) -> HashMap<String, XEntity> {
    let mut out = HashMap::new();
    let Some(items) = content_state
        .get("entity_map")
        .and_then(|value| value.as_array())
    else {
        return out;
    };
    for item in items {
        let Some(key) = item.get("key").and_then(x_json_key) else {
            continue;
        };
        let Some(value) = item.get("value") else {
            continue;
        };
        let kind = value
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        let data = value.get("data");
        let url = data
            .and_then(|data| data.get("url"))
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let media_ids = data
            .and_then(|data| data.get("media_items"))
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("media_id"))
                    .filter_map(x_json_key)
                    .collect()
            })
            .unwrap_or_default();
        out.insert(
            key,
            XEntity {
                kind,
                url,
                media_ids,
            },
        );
    }
    out
}

/// JSON 里的 key / id 可能是字符串，也可能是数字。
fn x_json_key(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|id| id.to_string()))
}

/// `media_id` → 图片地址（`media_entities` 的 `media_info.original_img_url`）。
fn x_article_media_urls(article: &serde_json::Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(items) = article
        .get("media_entities")
        .and_then(|value| value.as_array())
    else {
        return out;
    };
    for item in items {
        let Some(id) = item.get("media_id").and_then(x_json_key) else {
            continue;
        };
        let Some(info) = item.get("media_info") else {
            continue;
        };
        let url = [
            "/original_img_url",
            "/preview_image_url",
            "/media_url_https",
            "/url",
        ]
        .iter()
        .find_map(|path| info.pointer(path).and_then(|value| value.as_str()))
        .map(str::to_string);
        if let Some(url) = url {
            out.insert(id, url);
        }
    }
    out
}

/// `atomic` 块引用的实体（图片）。
fn x_block_entity<'a>(
    block: &serde_json::Value,
    entities: &'a HashMap<String, XEntity>,
) -> Option<&'a XEntity> {
    let ranges = block.get("entity_ranges")?.as_array()?;
    ranges.iter().find_map(|range| {
        let key = range.get("key")?;
        entities.get(&x_json_key(key)?)
    })
}

/// 一个行内标签（链接 / 粗体 / 斜体 / 删除线 / 下划线 / 代码）。
#[derive(Clone, PartialEq, Eq)]
enum XInlineTag {
    Link(String),
    Bold,
    Italic,
    Strike,
    Underline,
    Code,
}

impl XInlineTag {
    fn open(&self) -> String {
        match self {
            Self::Link(url) => format!("<a href=\"{}\">", escape_html(url)),
            Self::Bold => "<strong>".to_string(),
            Self::Italic => "<em>".to_string(),
            Self::Strike => "<del>".to_string(),
            Self::Underline => "<u>".to_string(),
            Self::Code => "<code>".to_string(),
        }
    }

    fn close(&self) -> &'static str {
        match self {
            Self::Link(_) => "</a>",
            Self::Bold => "</strong>",
            Self::Italic => "</em>",
            Self::Strike => "</del>",
            Self::Underline => "</u>",
            Self::Code => "</code>",
        }
    }
}

/// 把 Draft.js 的一段文本按链接实体与行内样式渲染成白名单 HTML。
fn render_x_inline(
    text: &str,
    block: &serde_json::Value,
    entities: &HashMap<String, XEntity>,
    base: Option<&reqwest::Url>,
) -> String {
    if text.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let links = x_link_ranges(block, &chars, entities, base);
    let styles = x_style_ranges(block, &chars);

    let mut out = String::with_capacity(text.len());
    let mut open: Vec<XInlineTag> = Vec::new();
    for (index, ch) in chars.iter().enumerate() {
        let mut desired = Vec::new();
        // 链接在最外层：Markdown 里是 `[**文字**](url)`。
        if let Some((_, _, url)) = links
            .iter()
            .find(|(start, end, _)| *start <= index && index < *end)
        {
            desired.push(XInlineTag::Link(url.clone()));
        }
        desired.extend(
            styles
                .iter()
                .filter(|(start, end, _)| *start <= index && index < *end)
                .map(|(_, _, tag)| tag.clone()),
        );

        let common = open
            .iter()
            .zip(desired.iter())
            .take_while(|(left, right)| left == right)
            .count();
        for tag in open.drain(common..).rev() {
            out.push_str(tag.close());
        }
        for tag in &desired[common..] {
            out.push_str(&tag.open());
            open.push(tag.clone());
        }
        out.push_str(&escape_html(&ch.to_string()));
    }
    for tag in open.iter().rev() {
        out.push_str(tag.close());
    }
    out
}

/// 链接实体覆盖的字符范围。
fn x_link_ranges(
    block: &serde_json::Value,
    chars: &[char],
    entities: &HashMap<String, XEntity>,
    base: Option<&reqwest::Url>,
) -> Vec<(usize, usize, String)> {
    let Some(ranges) = block
        .get("entity_ranges")
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };
    ranges
        .iter()
        .filter_map(|range| {
            let entity = entities.get(&x_json_key(range.get("key")?)?)?;
            if entity.kind != "LINK" {
                return None;
            }
            let url = absolute_url(base, entity.url.as_deref()?)?;
            let (start, end) = x_range_bounds(chars, range)?;
            Some((start, end, url))
        })
        .collect()
}

/// 行内样式覆盖的字符范围（按固定的嵌套顺序输出）。
fn x_style_ranges(block: &serde_json::Value, chars: &[char]) -> Vec<(usize, usize, XInlineTag)> {
    let Some(ranges) = block
        .get("inline_style_ranges")
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };
    let mut styles: Vec<(usize, usize, XInlineTag)> = ranges
        .iter()
        .filter_map(|range| {
            let style = range.get("style")?.as_str()?.to_ascii_lowercase();
            let tag = match style.as_str() {
                "bold" => XInlineTag::Bold,
                "italic" => XInlineTag::Italic,
                "strikethrough" => XInlineTag::Strike,
                "underline" => XInlineTag::Underline,
                "code" => XInlineTag::Code,
                _ => return None,
            };
            let (start, end) = x_range_bounds(chars, range)?;
            Some((start, end, tag))
        })
        .collect();
    // 固定嵌套顺序：粗体 → 斜体 → 删除线 → 下划线 → 代码。
    styles.sort_by_key(|(_, _, tag)| match tag {
        XInlineTag::Bold => 0,
        XInlineTag::Italic => 1,
        XInlineTag::Strike => 2,
        XInlineTag::Underline => 3,
        XInlineTag::Code => 4,
        XInlineTag::Link(_) => 5,
    });
    styles
}

/// Draft.js 的 offset/length 按 UTF-16 计，这里换成字符下标。
fn x_range_bounds(chars: &[char], range: &serde_json::Value) -> Option<(usize, usize)> {
    let offset = range.get("offset")?.as_u64()? as usize;
    let length = range.get("length")?.as_u64()? as usize;
    let start = x_utf16_to_char(chars, offset);
    let end = x_utf16_to_char(chars, offset + length);
    (start < end).then_some((start, end))
}

/// UTF-16 偏移 → 字符下标（emoji 等代理对不会把范围切错）。
fn x_utf16_to_char(chars: &[char], utf16_offset: usize) -> usize {
    let mut units = 0;
    for (index, ch) in chars.iter().enumerate() {
        if units >= utf16_offset {
            return index;
        }
        units += ch.len_utf16();
    }
    chars.len()
}

/// 帖子全文：长推文在 `note_tweet` 里，普通推文在 `legacy.full_text` 里。
fn x_tweet_text(tweet: &serde_json::Value) -> Option<String> {
    const PATHS: &[&str] = &[
        "/note_tweet/note_tweet_results/result/text",
        "/legacy/full_text",
        "/details/full_text",
        "/text",
    ];
    PATHS
        .iter()
        .find_map(|path| tweet.pointer(path).and_then(|value| value.as_str()))
        .map(str::to_string)
}

/// 帖子的作者字段（`core.user_results.result.core.{name,screen_name}`）。
fn x_user_field(tweet: &serde_json::Value, field: &str) -> Option<String> {
    let paths = [
        format!("/core/user_results/result/core/{field}"),
        format!("/core/user_results/result/legacy/{field}"),
        format!("/user/{field}"),
        format!("/core/{field}"),
    ];
    paths
        .iter()
        .find_map(|path| tweet.pointer(path).and_then(|value| value.as_str()))
        .map(str::to_string)
}

/// 帖子里的图片地址（`media_url_https`）；头像用的是 `profile_image_url_https`，不会混进来。
fn x_media_urls(tweet: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_media_urls(tweet, &mut out);
    out
}

fn collect_media_urls(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                // 引用帖的图片单独处理，不要混进主贴的图片列表。
                if matches!(
                    key.as_str(),
                    "quoted_tweet_results" | "quoted_tweet" | "quoted_status"
                ) {
                    continue;
                }
                if matches!(key.as_str(), "media_url_https" | "media_url") {
                    if let Some(src) = child.as_str().and_then(|src| absolute_url(None, src))
                        && !out.iter().any(|item| item == &src)
                    {
                        out.push(src);
                    }
                    continue;
                }
                collect_media_urls(child, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_media_urls(item, out);
            }
        }
        _ => {}
    }
}

/// JS 字符串反转义（X 的 SSR 数据是 JS 字符串里的 JSON）。
fn js_unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('u') => {
                push_unicode_escape(&mut chars, &mut out);
            }
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                if let Ok(code) = u32::from_str_radix(&hex, 16)
                    && let Some(ch) = char::from_u32(code)
                {
                    out.push(ch);
                }
            }
            // JS 的 `\'` 与其它转义：保留原字符。
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// 处理 `\uXXXX`（含代理对，例如 emoji）。
fn push_unicode_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, out: &mut String) {
    let hex: String = chars.by_ref().take(4).collect();
    let Ok(code) = u32::from_str_radix(&hex, 16) else {
        return;
    };
    if (0xD800..=0xDBFF).contains(&code) {
        // 高代理项：尝试拼上紧跟的低代理项。
        let mut probe = chars.clone();
        if probe.next() == Some('\\') && probe.next() == Some('u') {
            let low_hex: String = probe.by_ref().take(4).collect();
            if let Ok(low) = u32::from_str_radix(&low_hex, 16)
                && (0xDC00..=0xDFFF).contains(&low)
            {
                let combined = 0x1_0000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                if let Some(ch) = char::from_u32(combined) {
                    out.push(ch);
                    *chars = probe;
                    return;
                }
            }
        }
        out.push('\u{FFFD}');
        return;
    }
    if let Some(ch) = char::from_u32(code) {
        out.push(ch);
    }
}

/// 从 `text[start]` 的 `{` 开始，按括号配对切出一个完整的 JSON 对象。
fn json_object_slice(text: &str, start: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'{') {
        return None;
    }
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return text.get(start..=start + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// 帖子标题：正文第一行（去掉空行）；过长时截断。
fn smart_title_from_text(text: &str) -> Option<String> {
    let first = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    if first.is_empty() {
        return None;
    }
    Some(truncate_chars(first, 80))
}

/// 按空行把正文切成段落。
fn split_paragraphs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.trim().is_empty() {
                out.push(current.trim().to_string());
                current.clear();
            }
            continue;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line.trim_end());
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// 清掉 X/Twitter `<title>` 的 `作者 on X: "正文…"` 外壳。
fn clean_social_title(title: &str) -> String {
    let Some(tail) = title
        .strip_suffix(" / X")
        .or_else(|| title.strip_suffix(" / Twitter"))
    else {
        return title.to_string();
    };
    let Some((_, quoted)) = tail.split_once(": \"") else {
        return title.to_string();
    };
    let quoted = quoted.trim_end_matches('"').trim();
    smart_title_from_text(quoted).unwrap_or_else(|| title.to_string())
}

/// 正文块 → 字数、词数与摘要（`parse_html`、X 帖子与 Markdown 渲染共用）。
pub(crate) fn stats_from_blocks(blocks: &[ContentBlock]) -> (i64, i64, String) {
    let text = blocks
        .iter()
        .map(|block| plain_text(&block.html))
        .collect::<Vec<_>>();
    let chars = text.iter().map(|line| line.chars().count()).sum::<usize>() as i64;
    let words = text
        .iter()
        .map(|line| line.split_whitespace().count())
        .sum::<usize>() as i64;
    let excerpt = text
        .iter()
        .map(String::as_str)
        .find(|line| line.chars().count() >= 20)
        .map(|line| truncate_chars(line, 180))
        .unwrap_or_default();
    (chars, words, excerpt)
}

// ---------------------------------------------------------------------------
// DOM 遍历
// ---------------------------------------------------------------------------

/// 噪音节点：脚本、样式、导航等，整棵子树丢弃。
const NOISE_TAGS: &[&str] = &[
    "script", "style", "noscript", "iframe", "object", "embed", "svg", "canvas", "template",
    "form", "input", "button", "select", "textarea", "nav", "footer", "aside", "head", "link",
    "meta",
];

/// 能直接成为一块的标签。
const BLOCK_TAGS: &[&str] = &[
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "p",
    "blockquote",
    "pre",
    "figure",
    "ul",
    "ol",
    "table",
    "img",
    "dl",
    "figcaption",
    "details",
    "summary",
];

/// 行内白名单：保留语义，其余标签只保留其子节点。
const INLINE_TAGS: &[&str] = &[
    "a", "strong", "b", "em", "i", "code", "kbd", "samp", "var", "mark", "sup", "sub", "s", "u",
    "del", "ins", "small", "abbr", "q", "cite", "time",
];

/// 正文容器的 class 线索（命中任意一个就参与“文字最多”的比较）。
const CONTENT_CLASS_HINTS: &[&str] = &[
    "post",
    "article",
    "entry-content",
    "post-content",
    "article-content",
    "markdown-body",
    "content",
    // 微信文章正文（`<div id="js_content" class="rich_media_content …">`）。
    "rich_media_content",
];

/// 元素标签名（小写）；非元素返回 `None`。
fn tag_name(handle: &Handle) -> Option<&str> {
    match &handle.data {
        NodeData::Element { name, .. } => Some(&*name.local),
        _ => None,
    }
}

/// 取属性值（属性名大小写不敏感）。
fn attr(handle: &Handle, name: &str) -> Option<String> {
    let NodeData::Element { attrs, .. } = &handle.data else {
        return None;
    };
    attrs
        .borrow()
        .iter()
        .find(|attribute| attribute.name.local.as_ref().eq_ignore_ascii_case(name))
        .map(|attribute| attribute.value.to_string())
}

/// 是否是不可见元素：`display:none` 或 `hidden` 属性。
///
/// 微信文章（mdnice 导出）用小标题两侧的 `display:none` 的 `<span>unset</span>`
/// 做样式 hack，正文里会留下 “unsetunset” 噪音；不可见内容本来就不该进入正文。
fn is_hidden_element(handle: &Handle) -> bool {
    if attr(handle, "hidden").is_some() {
        return true;
    }
    attr(handle, "style").is_some_and(|style| {
        let compact: String = style
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        compact.contains("display:none")
    })
}

/// 自身或任一祖先是不可见元素（`find_all` 找到的图片可能藏在隐藏容器里）。
fn is_visually_hidden(handle: &Handle) -> bool {
    let mut current = Some(handle.clone());
    while let Some(node) = current {
        if is_hidden_element(&node) {
            return true;
        }
        // `parent` 是 `Cell<Option<Weak<Node>>>`：借出后立刻放回，避免改动文档结构。
        let parent = node.parent.take();
        let upgraded = parent.as_ref().and_then(|weak| weak.upgrade());
        node.parent.set(parent);
        current = upgraded;
    }
    false
}

/// 直接子元素。
fn child_elements(handle: &Handle) -> Vec<Handle> {
    handle
        .children
        .borrow()
        .iter()
        .filter(|child| matches!(&child.data, NodeData::Element { .. }))
        .cloned()
        .collect()
}

/// 全部后代元素（深度优先、文档顺序）。
fn descendants(handle: &Handle) -> Vec<Handle> {
    let mut out = Vec::new();
    let mut stack = vec![handle.clone()];
    while let Some(node) = stack.pop() {
        // 先收集子节点再压栈，保证输出顺序是文档顺序。
        let children = child_elements(&node);
        for child in children.iter().rev() {
            stack.push(child.clone());
        }
        if !Rc::ptr_eq(&node, handle) {
            out.push(node);
        }
    }
    out
}

/// 后代里第一个指定标签的元素。
fn find_first(handle: &Handle, tag: &str) -> Option<Handle> {
    descendants(handle)
        .into_iter()
        .find(|element| tag_name(element) == Some(tag))
}

/// 按 `id` 找元素（微信正文 `#js_content` 等）。
fn find_by_id(handle: &Handle, id: &str) -> Option<Handle> {
    descendants(handle)
        .into_iter()
        .find(|element| attr(element, "id").is_some_and(|value| value == id))
}

/// 后代里所有指定标签的元素。
fn find_all(handle: &Handle, tag: &str) -> Vec<Handle> {
    descendants(handle)
        .into_iter()
        .filter(|element| tag_name(element) == Some(tag))
        .collect()
}

/// 再往下找一层：`handle` 的子元素里标签匹配的那些。
fn direct_children_with_tag(handle: &Handle, tag: &str) -> Vec<Handle> {
    child_elements(handle)
        .into_iter()
        .filter(|element| tag_name(element) == Some(tag))
        .collect()
}

/// 元素内全部文本（含后代）。
fn text_content(handle: &Handle) -> String {
    let mut out = String::new();
    collect_text(handle, &mut out, 0);
    out
}

fn collect_text(handle: &Handle, out: &mut String, depth: usize) {
    if depth > MAX_DOM_DEPTH {
        return;
    }
    for child in handle.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => out.push_str(&contents.borrow()),
            NodeData::Element { .. } => {
                if tag_name(child).is_some_and(|tag| NOISE_TAGS.contains(&tag))
                    || is_hidden_element(child)
                {
                    continue;
                }
                collect_text(child, out, depth + 1);
            }
            _ => {}
        }
    }
}

/// 正文根节点：优先 `article` / `main` 等语义容器，取文字最多的一个；
/// 都不像正文时回退到 `<body>`。
fn content_root(document: &Handle) -> Option<Handle> {
    let mut candidates = Vec::new();
    collect_candidates(document, &mut candidates, 0);

    let best = candidates
        .into_iter()
        .map(|element| {
            let length = text_content(&element).chars().count();
            (element, length)
        })
        .max_by_key(|(_, length)| *length);

    match best {
        // 语义明确的正文容器（`<article>`、`#js_content` 等）再短也信；
        // 只按 class/id 猜出来的容器要过“文字够多”这一关，否则多半是导航/摘要。
        Some((element, length)) if length >= 200 || is_strong_content_element(&element) => {
            Some(element)
        }
        _ => find_first(document, "body").or_else(|| Some(document.clone())),
    }
}

/// 语义上就是正文的容器：不需要再用“文字够多”兜底。
fn is_strong_content_element(handle: &Handle) -> bool {
    let Some(tag) = tag_name(handle) else {
        return false;
    };
    if matches!(tag, "article" | "main") {
        return true;
    }
    if attr(handle, "role").is_some_and(|role| role.eq_ignore_ascii_case("main")) {
        return true;
    }
    let id_hit = attr(handle, "id").is_some_and(|id| id.eq_ignore_ascii_case("js_content"));
    let class_hit = attr(handle, "class").is_some_and(|class| {
        class
            .split_whitespace()
            .any(|name| name.eq_ignore_ascii_case("rich_media_content"))
    });
    id_hit || class_hit
}

fn collect_candidates(handle: &Handle, out: &mut Vec<Handle>, depth: usize) {
    if depth > MAX_DOM_DEPTH {
        return;
    }
    if is_content_element(handle) {
        out.push(handle.clone());
    }
    for child in child_elements(handle) {
        collect_candidates(&child, out, depth + 1);
    }
}

/// 判断 `handle` 是否像正文容器（标签、`role`、class、id 任一命中）。
fn is_content_element(handle: &Handle) -> bool {
    let Some(tag) = tag_name(handle) else {
        return false;
    };
    if matches!(tag, "article" | "main") {
        return true;
    }
    if attr(handle, "role").is_some_and(|role| role.eq_ignore_ascii_case("main")) {
        return true;
    }
    let class_hit = attr(handle, "class").is_some_and(|class| {
        class
            .split_whitespace()
            .any(|name| CONTENT_CLASS_HINTS.contains(&name.to_ascii_lowercase().as_str()))
    });
    let id_hit = attr(handle, "id").is_some_and(|id| {
        let id = id.to_ascii_lowercase();
        // `js_content` 是微信文章正文容器。
        matches!(id.as_str(), "content" | "main" | "article" | "js_content")
    });
    class_hit || id_hit
}

/// 递归收集块；容器只负责下钻，不生成块。
fn collect_blocks(
    parent: &Handle,
    base: Option<&reqwest::Url>,
    blocks: &mut Vec<ContentBlock>,
    images: &mut Vec<String>,
    depth: usize,
) {
    if depth > MAX_DOM_DEPTH {
        return;
    }
    for child in parent.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => {
                // 直接挂在容器上的文本（没有块级父元素）当成一段。
                let html = escape_html(&normalize_inline_text(&contents.borrow()));
                let html = html.trim();
                if !html.is_empty() {
                    blocks.push(block("p", format!("<p>{html}</p>")));
                }
            }
            NodeData::Element { .. } => {
                let Some(tag) = tag_name(child) else {
                    continue;
                };
                if NOISE_TAGS.contains(&tag) || is_hidden_element(child) {
                    continue;
                }
                if BLOCK_TAGS.contains(&tag) {
                    let tag = tag.to_string();
                    emit_block(child, &tag, base, blocks, images, depth + 1);
                } else {
                    collect_blocks(child, base, blocks, images, depth + 1);
                }
            }
            _ => {}
        }
    }
}

/// 把一个块级元素转成正文块（列表逐项、表格逐行、图片单独成块）。
fn emit_block(
    element: &Handle,
    tag: &str,
    base: Option<&reqwest::Url>,
    blocks: &mut Vec<ContentBlock>,
    images: &mut Vec<String>,
    depth: usize,
) {
    match tag {
        "pre" => {
            // `<pre><code>` 才让 htmd 输出围栏代码块（裸 `<pre>` 会被当成普通文本）。
            let code = text_content(element);
            if !code.trim().is_empty() {
                blocks.push(block(
                    "pre",
                    format!("<pre><code>{}</code></pre>", escape_html(&code)),
                ));
            }
        }
        "ul" | "ol" => {
            // 整张列表作为一个块：转 Markdown 时才能得到真正的 `- item` / `1. item`。
            let mut html = format!("<{tag}>");
            for item in list_items(element) {
                let inner = render_inline_children(&item, base, images, depth);
                let inner = inner.trim();
                if !inner.is_empty() {
                    html.push_str(&format!("<li>{inner}</li>"));
                }
            }
            html.push_str(&format!("</{tag}>"));
            if html != format!("<{tag}></{tag}>") {
                blocks.push(block(tag, html));
            }
        }
        "table" => {
            // 整张表一个块：`htmd` 才能输出 Markdown 表格（第一行当表头）。
            let rows: Vec<Vec<String>> = table_rows(element)
                .into_iter()
                .map(|row| {
                    row.children
                        .borrow()
                        .iter()
                        .filter(|cell| tag_name(cell).is_some_and(|tag| tag == "td" || tag == "th"))
                        .map(|cell| {
                            render_inline_children(cell, base, images, depth)
                                .trim()
                                .to_string()
                        })
                        .collect()
                })
                .filter(|row: &Vec<String>| row.iter().any(|cell| !cell.is_empty()))
                .collect();
            if let Some((header, body)) = rows.split_first() {
                let mut html = String::from("<table><thead><tr>");
                for cell in header {
                    html.push_str(&format!("<th>{cell}</th>"));
                }
                html.push_str("</tr></thead><tbody>");
                for row in body {
                    html.push_str("<tr>");
                    for cell in row {
                        html.push_str(&format!("<td>{cell}</td>"));
                    }
                    html.push_str("</tr>");
                }
                html.push_str("</tbody></table>");
                blocks.push(block("table", html));
            }
        }
        "figure" | "img" => {
            if tag == "img" {
                push_image(element, base, blocks, images);
            }
            for image in find_all(element, "img") {
                push_image(&image, base, blocks, images);
            }
            for caption in find_all(element, "figcaption") {
                let inner = render_inline_children(&caption, base, images, depth);
                let inner = inner.trim();
                if !inner.is_empty() {
                    blocks.push(block("p", format!("<p>{inner}</p>")));
                }
            }
        }
        "dl" => {
            for child in child_elements(element) {
                if !tag_name(&child).is_some_and(|tag| tag == "dt" || tag == "dd") {
                    continue;
                }
                let inner = render_inline_children(&child, base, images, depth);
                if !inner.trim().is_empty() {
                    blocks.push(block("p", format!("<p>{}</p>", inner.trim())));
                }
            }
        }
        "figcaption" | "details" | "summary" => {
            let inner = render_inline_children(element, base, images, depth);
            if !inner.trim().is_empty() {
                blocks.push(block("p", format!("<p>{}</p>", inner.trim())));
            }
        }
        _ => {
            let inner = render_inline_children(element, base, images, depth);
            let inner = inner.trim();
            if !inner.is_empty() {
                blocks.push(block(tag, format!("<{tag}>{inner}</{tag}>")));
            }
        }
    }
}

/// 表格的行：只取本表的行（`thead`/`tbody`/`tfoot` 的直接 `<tr>`），
/// 不把嵌套表格的行拍平进来——那会造出几十列的畸形 Markdown 表格。
fn table_rows(table: &Handle) -> Vec<Handle> {
    let mut rows = Vec::new();
    for child in child_elements(table) {
        match tag_name(&child) {
            Some("thead" | "tbody" | "tfoot") => {
                rows.extend(direct_children_with_tag(&child, "tr"));
            }
            Some("tr") => rows.push(child),
            _ => {}
        }
    }
    rows
}

/// 列表项：只取直接的 `<li>`（子列表由阅读器当成独立段落，不在这里嵌套）。
fn list_items(list: &Handle) -> Vec<Handle> {
    let mut items = direct_children_with_tag(list, "li");
    if items.is_empty() {
        // 有的站点写成 `<ul><div><li>`：退回后代查询。
        items = find_all(list, "li");
    }
    items
}

/// 取图片的真实地址：优先 `src`，跳过 `data:` 占位图后回退到常见懒加载属性。
///
/// 微信、知乎等站点把正文图片放在 `data-src` 里，`src` 只是一个 `data:` 占位图；
/// 只认 `src` 会把整篇文章的图都丢掉。
fn image_source(image: &Handle, base: Option<&reqwest::Url>) -> Option<String> {
    const ATTRIBUTES: &[&str] = &[
        "src",
        "data-src",
        "data-original",
        "data-lazy-src",
        "data-actualsrc",
    ];
    ATTRIBUTES
        .iter()
        .find_map(|name| attr(image, name).and_then(|raw| absolute_url(base, &raw)))
}

/// 图片块：只接受 http/https 绝对地址。
///
/// `data:` 内联图与相对路径不参与离线缓存，直接跳过（正文里仍留下占位说明）。
fn push_image(
    image: &Handle,
    base: Option<&reqwest::Url>,
    blocks: &mut Vec<ContentBlock>,
    images: &mut Vec<String>,
) -> bool {
    if is_visually_hidden(image) {
        return false;
    }
    let Some(src) = image_source(image, base) else {
        return false;
    };
    let alt = attr(image, "alt")
        .map(|alt| collapse_whitespace(&alt))
        .unwrap_or_default();
    if !images.iter().any(|item| item == &src) {
        images.push(src.clone());
    }
    blocks.push(block(
        "figure",
        format!(
            "<p><img src=\"{}\" alt=\"{}\"></p>",
            escape_html(&src),
            escape_html(&alt)
        ),
    ));
    true
}

/// 把一个元素的行内内容渲染成白名单 HTML。
fn render_inline_children(
    element: &Handle,
    base: Option<&reqwest::Url>,
    images: &mut Vec<String>,
    depth: usize,
) -> String {
    let mut out = String::new();
    for child in element.children.borrow().iter() {
        render_inline_node(child, base, images, &mut out, depth + 1);
    }
    out
}

/// 递归渲染一个子节点（文本或元素）。
fn render_inline_node(
    node: &Handle,
    base: Option<&reqwest::Url>,
    images: &mut Vec<String>,
    out: &mut String,
    depth: usize,
) {
    if depth > MAX_DOM_DEPTH {
        return;
    }
    match &node.data {
        NodeData::Text { contents } => {
            out.push_str(&escape_html(&normalize_inline_text(&contents.borrow())));
        }
        NodeData::Element { .. } => {
            let Some(name) = tag_name(node) else {
                return;
            };
            if NOISE_TAGS.contains(&name) || is_hidden_element(node) {
                return;
            }
            if name == "br" {
                out.push_str("<br>");
                return;
            }
            if name == "img" {
                if let Some(src) = image_source(node, base) {
                    let alt = attr(node, "alt")
                        .map(|alt| collapse_whitespace(&alt))
                        .unwrap_or_default();
                    if !images.iter().any(|item| item == &src) {
                        images.push(src.clone());
                    }
                    out.push_str(&format!(
                        "<img src=\"{}\" alt=\"{}\">",
                        escape_html(&src),
                        escape_html(&alt)
                    ));
                }
                return;
            }
            if name == "a" {
                match attr(node, "href").and_then(|href| absolute_url(base, &href)) {
                    Some(href) => {
                        out.push_str(&format!("<a href=\"{}\">", escape_html(&href)));
                        for child in node.children.borrow().iter() {
                            render_inline_node(child, base, images, out, depth + 1);
                        }
                        out.push_str("</a>");
                    }
                    None => {
                        for child in node.children.borrow().iter() {
                            render_inline_node(child, base, images, out, depth + 1);
                        }
                    }
                }
                return;
            }
            if INLINE_TAGS.contains(&name) {
                out.push_str(&format!("<{name}>"));
                for child in node.children.borrow().iter() {
                    render_inline_node(child, base, images, out, depth + 1);
                }
                out.push_str(&format!("</{name}>"));
                return;
            }
            // 其余标签（div/span/未知）只保留文字内容。
            for child in node.children.borrow().iter() {
                render_inline_node(child, base, images, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// 相对地址 → 绝对地址；只接受 http/https。
fn absolute_url(base: Option<&reqwest::Url>, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with("data:") || raw.starts_with('#') {
        return None;
    }
    let joined = match base {
        Some(base) => base.join(raw).ok()?,
        None => reqwest::Url::parse(raw).ok()?,
    };
    match joined.scheme() {
        "http" | "https" => Some(joined.to_string()),
        _ => None,
    }
}

fn extract_title(document: &Handle, root: Option<&Handle>, base: Option<&reqwest::Url>) -> String {
    // 1) og:title / twitter:title
    for meta in find_all(document, "meta") {
        let key = attr(&meta, "property").or_else(|| attr(&meta, "name"));
        let Some(key) = key else { continue };
        if matches!(
            key.to_ascii_lowercase().as_str(),
            "og:title" | "twitter:title"
        ) && let Some(content) = attr(&meta, "content")
        {
            let title = collapse_whitespace(&content);
            if !title.is_empty() {
                return title;
            }
        }
    }

    // 2) <title>
    if let Some(title) = find_first(document, "title") {
        let title = collapse_whitespace(&text_content(&title));
        if !title.is_empty() {
            return title;
        }
    }

    // 3) 正文里的第一个 h1
    if let Some(root) = root
        && let Some(heading) = find_first(root, "h1")
    {
        let title = collapse_whitespace(&text_content(&heading));
        if !title.is_empty() {
            return title;
        }
    }

    // 4) 主机名兜底
    base.and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "未命名网页".to_string())
}

fn extract_site(document: &Handle, base: Option<&reqwest::Url>) -> String {
    for meta in find_all(document, "meta") {
        let key = attr(&meta, "property").or_else(|| attr(&meta, "name"));
        let Some(key) = key else { continue };
        if matches!(
            key.to_ascii_lowercase().as_str(),
            "og:site_name" | "application-name"
        ) && let Some(content) = attr(&meta, "content")
        {
            let site = collapse_whitespace(&content);
            if !site.is_empty() {
                return site;
            }
        }
    }
    base.and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default()
}

/// 构造一个正文块。
fn block(tag: &str, html: String) -> ContentBlock {
    ContentBlock {
        tag: tag.to_string(),
        html,
        leaves: Vec::new(),
    }
}

/// 块 HTML → 纯文本（只用于统计字数与摘要，渲染仍用 HTML）。
fn plain_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..lt]));
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else {
            break;
        };
        rest = &rest[gt + 1..];
    }
    out.push_str(&decode_entities(rest));
    collapse_whitespace(&out)
}

pub(crate) fn reading_minutes(words: i64) -> i64 {
    // 按 200 词/分钟的常见口径估算；有正文时至少 1 分钟。
    if words <= 0 { 0 } else { (words + 199) / 200 }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= limit {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

/// HTML 转义（正文文本写回块 HTML 时用）。
pub(crate) fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// 解码常见 HTML 实体（只用于统计/摘要；解析后的文本节点已经解码过）。
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|dec| dec.parse::<u32>().ok())
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 行内文本归一化：空白折叠成一个空格，**保留**首尾空格（词与词之间不粘连）。
fn normalize_inline_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    if pending_space {
        out.push(' ');
    }
    out
}

/// 空白归一化（去首尾）：用于标题、属性、统计文本。
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------------------
// 网络
// ---------------------------------------------------------------------------

/// 按站点选 UA：微信文章必须用微信内 UA，其它站点用带项目标识的默认 UA。
fn user_agent_for(url: Option<&reqwest::Url>) -> &'static str {
    match url.and_then(reqwest::Url::host_str) {
        Some(host) if is_weixin_host(host) => WEIXIN_USER_AGENT,
        _ => USER_AGENT,
    }
}

fn referer_for(url: Option<&reqwest::Url>) -> Option<&'static str> {
    match url.and_then(reqwest::Url::host_str) {
        Some(host) if is_weixin_host(host) => Some(WEIXIN_REFERER),
        _ => None,
    }
}

/// 微信文章站点（不含图片 CDN）：只有这些主机的页面才用 `#js_content` 特例。
fn is_weixin_article_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "mp.weixin.qq.com" || host == "weixin.qq.com" || host.ends_with(".weixin.qq.com")
}

fn is_weixin_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "mp.weixin.qq.com"
        || host == "weixin.qq.com"
        || host.ends_with(".weixin.qq.com")
        // 微信图片 CDN：带上微信 UA 与 Referer，避免防盗链拿到占位图。
        || host.ends_with(".qpic.cn")
}

fn web_client(url: Option<&reqwest::Url>) -> Result<reqwest::blocking::Client, TelaError> {
    reqwest::blocking::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent(user_agent_for(url))
        .build()
        .map_err(|error| config(format!("无法创建网页下载客户端：{error}")))
}

/// 读取响应体文本：最多 `max` 字节，按 UTF-8 宽松解码（截断处不会 panic）。
///
/// 注意：有些站点（包括微信文章）把正文发完后会直接断开 TLS，`read_to_end` 会报
/// 「unexpected eof」。已经收到的字节仍然是完整的 HTML 前缀，这里保留它们继续解析，
/// 只在「一个字节都没收到」时才算失败。
fn read_text(mut response: reqwest::blocking::Response, max: usize) -> Result<String, TelaError> {
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        if buffer.len() >= max {
            break;
        }
        match response.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                let take = read.min(max - buffer.len());
                buffer.extend_from_slice(&chunk[..take]);
            }
            // 连接被对端提前关闭：保留已读内容，交给解析层判断是否有正文。
            Err(_) if !buffer.is_empty() => break,
            Err(error) => return Err(read_error("读取网页内容", error)),
        }
    }
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

fn read_bytes(response: reqwest::blocking::Response, max: usize) -> Result<Vec<u8>, TelaError> {
    let mut buffer = Vec::new();
    let mut limited = response.take(max as u64 + 1);
    limited
        .read_to_end(&mut buffer)
        .map_err(|error| read_error("读取图片内容", error))?;
    if buffer.len() > max {
        buffer.truncate(max);
    }
    Ok(buffer)
}

fn config(detail: impl Into<String>) -> TelaError {
    TelaError::Config {
        detail: detail.into(),
    }
}

fn net_error(url: &str, error: impl std::fmt::Display) -> TelaError {
    TelaError::Net {
        ctx: WireContext::new(url, "webpage"),
        detail: error.to_string(),
    }
}

fn read_error(what: &str, error: impl std::fmt::Display) -> TelaError {
    TelaError::Net {
        ctx: WireContext::new("webpage", "read"),
        detail: format!("{what}失败：{error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_url_adds_scheme_and_rejects_others() {
        assert_eq!(
            normalize_url("example.com/post").unwrap(),
            "https://example.com/post"
        );
        assert_eq!(
            normalize_url("  https://example.com/a?b=1  ").unwrap(),
            "https://example.com/a?b=1"
        );
        assert!(normalize_url("javascript:alert(1)").is_err());
        assert!(normalize_url("file:///etc/passwd").is_err());
        assert!(normalize_url("   ").is_err());
    }

    #[test]
    fn content_key_is_stable_and_prefixed() {
        let a = content_key_for_url("https://example.com/a");
        assert_eq!(a, content_key_for_url("https://example.com/a"));
        assert_ne!(a, content_key_for_url("https://example.com/b"));
        assert!(a.starts_with(WEB_CONTENT_KEY_PREFIX));
        assert!(is_local_page_key(&a));
    }

    #[test]
    fn whitespace_collapse_keeps_single_spaces() {
        assert_eq!(collapse_whitespace("a \n\t b\r\n c"), "a b c");
    }

    #[test]
    fn inline_text_keeps_word_boundaries() {
        assert_eq!(normalize_inline_text("Hello "), "Hello ");
        assert_eq!(normalize_inline_text(" !"), " !");
        assert_eq!(normalize_inline_text("a\n b"), "a b");
    }

    #[test]
    fn escape_html_escapes_special_characters() {
        assert_eq!(escape_html("<a & b>"), "&lt;a &amp; b&gt;");
    }

    #[test]
    fn x_status_urls_are_recognised() {
        assert!(is_x_status_url(
            "https://x.com/UPing123zzz/status/2116729183334523014"
        ));
        assert!(is_x_status_url("https://twitter.com/ethan/status/1?s=20"));
        assert!(is_x_status_url("https://mobile.x.com/ethan/status/1"));
        assert!(!is_x_status_url("https://x.com/home"));
        assert!(!is_x_status_url("https://x.com/ethan"));
        assert!(!is_x_status_url("https://example.com/ethan/status/1"));
    }

    #[test]
    fn x_article_urls_are_recognised() {
        assert!(is_x_article_url(
            "https://x.com/SuJinyan6/article/2116730566678868190"
        ));
        assert!(is_x_article_url("https://twitter.com/ethan/article/1?s=20"));
        assert!(is_x_article_url(
            "https://x.com/i/article/2116723416606924800"
        ));
        assert!(!is_x_article_url("https://x.com/ethan/status/1"));
        assert!(!is_x_article_url("https://x.com/ethan"));
        assert!(!is_x_article_url("https://example.com/ethan/article/1"));

        assert!(is_x_post_url(
            "https://x.com/SuJinyan6/article/2116730566678868190"
        ));
        assert!(is_x_post_url(
            "https://x.com/UPing123zzz/status/2116729183334523014"
        ));
        assert!(!is_x_post_url("https://example.com/ethan/article/1"));
    }

    #[test]
    fn x_posts_use_the_first_text_line_as_title_and_embed_quoted_tweets() {
        let json = "{\"data\":{\"tweet_result_by_rest_id\":{\"id\":\"x\",\
            \"rest_id\":\"123\",\"result\":{\"__typename\":\"Tweet\",\
            \"core\":{\"user_results\":{\"result\":{\"core\":{\"name\":\"Ethan\",\
            \"screen_name\":\"ethan\"}}}},\"legacy\":{\"lang\":\"zh\"},\
            \"note_tweet\":{\"note_tweet_results\":{\"result\":{\
            \"__typename\":\"NoteTweet\",\"text\":\"第一行标题\\n\\n第二段\\n\\n第三段\"}}},\
            \"quoted_tweet_results\":{\"result\":{\"__typename\":\"Tweet\",\
            \"core\":{\"user_results\":{\"result\":{\"core\":{\"name\":\"朱峰\",\
            \"screen_name\":\"zhufeng\"}}}},\"note_tweet\":{\"note_tweet_results\":{\
            \"result\":{\"text\":\"引用正文\"}}}}}}}}}";
        // X 把 JSON 放在 JS 字符串里：引号与反斜杠都要再转义一层。
        let escaped = json.replace('\\', "\\\\").replace('"', "\\\"");
        let html = format!(
            "<html><body><script nonce=\"x\">$_TSR.router=($R=>$R[0]={{value:\"{escaped}\"}})</script>             <p>Sign up</p></body></html>"
        );

        let page = x_post_from_html("https://x.com/ethan/status/123", &html)
            .expect("should extract an X post");
        assert_eq!(page.title, "第一行标题");
        assert_eq!(page.site, "X");
        assert_eq!(page.lang, "zh");

        let markdown = page.markdown;
        assert!(markdown.contains("Ethan（@ethan）"), "{markdown}");
        assert!(markdown.contains("第二段"), "{markdown}");
        assert!(markdown.contains("第三段"), "{markdown}");
        assert!(
            markdown.contains("引用正文"),
            "引用帖应作为引用块：{markdown}"
        );
        assert!(markdown.contains("朱峰（@zhufeng）"), "{markdown}");
        assert!(!markdown.contains("Sign up"), "不应混入页面 UI：{markdown}");
    }

    /// 把若干 SSR JSON 值包进一个 X 风格的脚本里（数据在 JS 字符串里）。
    fn x_html(values: &[serde_json::Value]) -> String {
        let parts: Vec<String> = values
            .iter()
            .map(|value| {
                let escaped = serde_json::to_string(value)
                    .unwrap()
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"");
                format!("{{value:\"{escaped}\"}}")
            })
            .collect();
        format!(
            "<html><body><script nonce=\"x\">$_TSR.router=($R=>$R[0]=[{}])</script>\
             <p>Sign up</p></body></html>",
            parts.join(",")
        )
    }

    fn x_article_json(content_state: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "data": {
                "tweet_result_by_rest_id": {
                    "id": "x",
                    "rest_id": "123",
                    "result": {
                        "__typename": "Tweet",
                        "core": {
                            "user_results": {
                                "result": {
                                    "core": {"name": "Ethan", "screen_name": "ethan"}
                                }
                            }
                        },
                        "legacy": {"lang": "zh"},
                        "article": {
                            "article_results": {
                                "result": {
                                    "__typename": "ArticleEntity",
                                    "title": "文章标题",
                                    "plain_text": "第一段\n第二段",
                                    "content_state": content_state,
                                    "media_entities": [{
                                        "media_id": "m1",
                                        "media_info": {
                                            "__typename": "ApiImage",
                                            "original_img_url": "https://pbs.twimg.com/media/pic.jpg"
                                        }
                                    }],
                                    "metadata": {}
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    #[test]
    fn x_articles_use_the_article_title_and_draft_blocks() {
        let content_state = serde_json::json!({
            "blocks": [
                {
                    "type": "unstyled",
                    "text": "开头 粗体 链接",
                    "entity_ranges": [{"key": 0, "offset": 6, "length": 2}],
                    "inline_style_ranges": [{"offset": 0, "length": 2, "style": "Bold"}]
                },
                {"type": "header-two", "text": "小标题", "entity_ranges": [], "inline_style_ranges": []},
                {"type": "unordered-list-item", "text": "第一项", "entity_ranges": [], "inline_style_ranges": []},
                {"type": "unordered-list-item", "text": "第二项", "entity_ranges": [], "inline_style_ranges": []},
                {"type": "atomic", "text": " ", "entity_ranges": [{"key": 1, "offset": 0, "length": 1}], "inline_style_ranges": []},
                {"type": "unstyled", "text": "图片之后", "entity_ranges": [], "inline_style_ranges": []}
            ],
            "entity_map": [
                {"key": "0", "value": {"type": "LINK", "data": {"url": "https://example.com/start"}}},
                {"key": "1", "value": {"type": "MEDIA", "data": {"media_items": [{"media_id": "m1"}]}}}
            ]
        });
        let html = x_html(&[x_article_json(content_state)]);

        let page = x_post_from_html("https://x.com/ethan/article/123", &html)
            .expect("should extract an X article");
        assert_eq!(page.title, "文章标题");
        assert_eq!(page.site, "X");
        assert_eq!(page.lang, "zh");
        assert_eq!(page.images, vec!["https://pbs.twimg.com/media/pic.jpg"]);
        assert!(page.chars > 20, "正文应有内容：{}", page.chars);

        let markdown = page.markdown;
        assert!(markdown.contains("Ethan（@ethan）"), "{markdown}");
        assert!(markdown.contains("**开头**"), "粗体应保留：{markdown}");
        assert!(
            markdown.contains("[链接](https://example.com/start)"),
            "链接应保留并改写成绝对地址：{markdown}"
        );
        assert!(
            markdown.contains("## 小标题"),
            "小标题应成为 Markdown 标题：{markdown}"
        );
        assert!(markdown.contains("第一项"), "列表应保留：{markdown}");
        assert!(markdown.contains("第二项"), "列表应保留：{markdown}");
        assert!(
            markdown.contains("![图片]") || markdown.contains("<img"),
            "图片应进入正文：{markdown}"
        );
        assert!(markdown.contains("图片之后"), "{markdown}");
        assert!(!markdown.contains("Sign up"), "不应混入页面 UI：{markdown}");
    }

    #[test]
    fn x_articles_win_over_the_card_copy_in_the_same_page() {
        // 状态页里先出现文章卡（只有标题/摘要），后面才是带 content_state 的完整结果。
        let card = serde_json::json!({
            "data": {
                "tweet_result_by_rest_id": {
                    "id": "x",
                    "rest_id": "123",
                    "result": {
                        "__typename": "Tweet",
                        "article": {
                            "article_results": {
                                "result": {
                                    "__typename": "ArticleEntity",
                                    "title": "卡片标题",
                                    "preview_text": "只有摘要",
                                    "id": "QXJ0aWNsZUVudGl0eTox",
                                    "rest_id": "1"
                                }
                            }
                        }
                    }
                }
            }
        });
        let content_state = serde_json::json!({
            "blocks": [
                {"type": "unstyled", "text": "完整正文", "entity_ranges": [], "inline_style_ranges": []}
            ],
            "entity_map": []
        });
        // 时间线里另一篇文章的完整结果：不按 URL 的 rest_id 过滤就会被它抢走。
        let mut other = x_article_json(content_state.clone());
        other["data"]["tweet_result_by_rest_id"]["rest_id"] = serde_json::json!("999");
        other["data"]["tweet_result_by_rest_id"]["result"]["article"]["article_results"]["result"]
            ["title"] = serde_json::json!("别的文章");
        other["data"]["tweet_result_by_rest_id"]["result"]["article"]["article_results"]["result"]
            ["content_state"]["blocks"][0]["text"] = serde_json::json!("别的正文");
        let html = x_html(&[card, other, x_article_json(content_state)]);

        let page = x_post_from_html("https://x.com/ethan/status/123", &html)
            .expect("should pick the complete article payload");
        assert_eq!(page.title, "文章标题");
        assert!(page.markdown.contains("完整正文"), "{}", page.markdown);
        assert!(!page.markdown.contains("只有摘要"), "{}", page.markdown);
        assert!(!page.markdown.contains("别的正文"), "{}", page.markdown);
    }

    #[test]
    fn x_login_gated_article_links_are_reported_as_blocked() {
        let detail = x_login_gate_detail(
            "https://x.com/i/jf/onboarding/web?redirect_after_login=%2Fi%2Farticle%2F2116723416606924800&mode=login",
        )
        .expect("should recognise the article login gate");
        assert!(detail.contains("文章页"), "{detail}");
        assert!(
            x_login_gate_detail("https://x.com/SuJinyan6/article/2116730566678868190").is_none()
        );
        assert!(x_login_gate_detail("https://example.com/i/article/1").is_none());
    }

    #[test]
    fn weixin_hosts_use_the_wechat_user_agent_and_referer() {
        let wechat = reqwest::Url::parse("https://mp.weixin.qq.com/s/abc?scene=1").unwrap();
        assert_eq!(user_agent_for(Some(&wechat)), WEIXIN_USER_AGENT);
        assert_eq!(referer_for(Some(&wechat)), Some(WEIXIN_REFERER));

        let image = reqwest::Url::parse("https://mmbiz.qpic.cn/mmbiz_png/abc/640").unwrap();
        assert_eq!(user_agent_for(Some(&image)), WEIXIN_USER_AGENT);
        assert_eq!(referer_for(Some(&image)), Some(WEIXIN_REFERER));

        let other = reqwest::Url::parse("https://example.com/post").unwrap();
        assert_eq!(user_agent_for(Some(&other)), USER_AGENT);
        assert_eq!(referer_for(Some(&other)), None);
    }

    #[test]
    fn reading_minutes_is_never_zero_for_text() {
        assert_eq!(reading_minutes(0), 0);
        assert_eq!(reading_minutes(1), 1);
        assert_eq!(reading_minutes(201), 2);
    }
}
