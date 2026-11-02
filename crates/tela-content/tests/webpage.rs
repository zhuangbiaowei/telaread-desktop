//! 网页收藏的抓取/抽取测试（纯函数，不联网）。
//!
//! 覆盖：Readability 正文抽取、标题/站点/语言、噪音节点丢弃、白名单 HTML、
//! Markdown 输出（标题/列表/表格/代码/链接/图片）、隐藏节点、懒加载图片、
//! 微信与 X 站点适配、验证页识别。

use tela_content::webpage::{content_key_for_url, parse_html};

const FIXTURE: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>Fallback Title</title>
    <meta property="og:title" content="  Reading Tela  ">
    <meta property="og:site_name" content="Example  Blog">
    <script>window.__secret = "should not appear";</script>
    <style>.post { color: red; }</style>
  </head>
  <body>
    <nav><a href="/">Home</a> | <a href="/about">About</a></nav>
    <article class="post">
      <h1>Reading Tela</h1>
      <p>Hello <strong>world</strong>, read the <a href="/docs/start">quick start</a> now.</p>
      <p>Second paragraph with a <a href="javascript:alert(1)">bad link</a> and an
         <img src="data:image/png;base64,AAAA" alt="inline"></p>
      <ul>
        <li>First item</li>
        <li>Second <em>item</em></li>
      </ul>
      <pre><code>fn main() {
    println!("hi");
}</code></pre>
      <figure>
        <img src="../images/cover.png" alt="Cover">
        <figcaption>The cover figure</figcaption>
      </figure>
      <table>
        <tr><th>Name</th><th>Value</th></tr>
        <tr><td>alpha</td><td>1</td></tr>
      </table>
      <blockquote>Quoted text</blockquote>
      <p onclick="steal()">Clickable paragraph</p>
    </article>
    <footer>Footer text</footer>
    <aside>Aside text</aside>
  </body>
</html>"#;

#[test]
fn extracts_title_site_language_and_markdown() {
    let page = parse_html("https://example.com/posts/reading-tela", FIXTURE);

    assert_eq!(page.title, "Reading Tela");
    assert_eq!(page.site, "Example Blog");
    assert_eq!(page.lang, "en");
    assert!(!page.is_empty());
    assert!(page.chars > 0);
    assert!(page.words > 0);

    let markdown = &page.markdown;
    // Readability 会把标题从正文里摘到 `page.title`，正文只保留段落与强调。
    assert!(
        markdown.contains("Hello **world**"),
        "段落与强调应进入 Markdown：{markdown}"
    );
    assert!(markdown.contains("Clickable paragraph"), "{markdown}");
    // 列表、代码块、表格、引用、图片。
    assert!(markdown.contains("First item"), "{markdown}");
    assert!(markdown.contains("```"), "代码块应是围栏式：{markdown}");
    assert!(markdown.contains("println!"));
    assert!(
        markdown.contains("| Name"),
        "表格应是 Markdown 表格：{markdown}"
    );
    assert!(markdown.contains("alpha"), "{markdown}");
    assert!(markdown.contains("Quoted text"), "{markdown}");
    assert!(
        markdown.contains("![Cover]"),
        "图片应是 Markdown 图片：{markdown}"
    );

    // 噪音节点（脚本/样式/导航/页脚/侧栏）与危险属性不进入正文。
    assert!(!markdown.contains("__secret"));
    assert!(!markdown.contains("color: red"));
    assert!(!markdown.contains("Home"));
    assert!(!markdown.contains("Footer text"));
    assert!(!markdown.contains("Aside text"));
    assert!(!markdown.contains("steal()"), "事件属性必须丢弃");
    assert!(!markdown.contains("javascript:"), "非 http(s) 链接必须丢弃");
    assert!(!markdown.contains("data:image"), "内联图不进入正文");
}

#[test]
fn rewrites_links_and_images_to_absolute_urls() {
    let page = parse_html("https://example.com/posts/reading-tela", FIXTURE);

    let markdown = &page.markdown;
    assert!(
        markdown.contains("(https://example.com/docs/start)"),
        "相对链接应改写为绝对地址：{markdown}"
    );
    assert!(
        markdown.contains("(https://example.com/images/cover.png)"),
        "相对图片应改写为绝对地址：{markdown}"
    );
    assert_eq!(
        page.images,
        vec!["https://example.com/images/cover.png".to_string()]
    );
}

#[test]
fn code_blocks_keep_whitespace_and_are_fenced() {
    let page = parse_html("https://example.com/posts/reading-tela", FIXTURE);
    let markdown = &page.markdown;
    let code_start = markdown.find("```").expect("有围栏代码块");
    let code = &markdown[code_start..];
    assert!(code.contains("println!"));
    assert!(code.contains("\n    "), "代码缩进应保留：{code}");
}

#[test]
fn code_blocks_are_not_link_rewritten() {
    let html = r#"<html><body><article>
        <pre><code>示例：![x](/relative.png) 与 [y](/relative)</code></pre>
        <p><a href="/docs/a">链接</a></p>
    </article></body></html>"#;
    let page = parse_html("https://example.com/post", html);
    assert!(
        page.markdown.contains("](/relative.png)"),
        "代码块里的示例不能被改写：{}",
        page.markdown
    );
    assert!(
        page.markdown.contains("(https://example.com/docs/a)"),
        "正文链接仍应改写：{}",
        page.markdown
    );
}

#[test]
fn falls_back_to_title_element_then_h1_then_host() {
    let page = parse_html(
        "https://example.com/x",
        "<html><head><title>  Only Title  </title></head><body><p>text</p></body></html>",
    );
    assert_eq!(page.title, "Only Title");

    let page = parse_html(
        "https://example.com/x",
        "<html><body><article><h1>Heading Title</h1><p>body text</p></article></body></html>",
    );
    assert_eq!(page.title, "Heading Title");

    let page = parse_html("https://example.com/x", "<html><body></body></html>");
    assert_eq!(page.title, "example.com");
    assert!(page.is_empty());
}

#[test]
fn prefers_the_largest_article_container() {
    let html = format!(
        r#"<html><body>
        <main><p>short teaser</p></main>
        <article><p>{}</p><p>second</p></article>
        </body></html>"#,
        "long article text. ".repeat(20)
    );
    let page = parse_html("https://example.com/a", &html);
    assert!(
        page.markdown.contains("long article text"),
        "{}",
        page.markdown
    );
    assert!(!page.markdown.contains("short teaser"), "{}", page.markdown);
}

#[test]
fn content_key_is_stable_and_prefixed() {
    let a = content_key_for_url("https://example.com/a");
    assert_eq!(a, content_key_for_url("https://example.com/a"));
    assert_ne!(a, content_key_for_url("https://example.com/b"));
    assert!(a.starts_with("web-"));
    assert!(tela_content::webpage::is_local_page_key(&a));
}

#[test]
fn excerpt_is_a_readable_prefix_of_the_first_paragraph() {
    let page = parse_html("https://example.com/posts/reading-tela", FIXTURE);
    assert!(page.excerpt.contains("Hello world"), "{:?}", page.excerpt);
    assert!(!page.excerpt.contains('<'));
    assert!(page.excerpt.chars().count() <= 181);
}

#[test]
fn empty_body_yields_empty_page_without_panicking() {
    let page = parse_html("https://example.com/x", "");
    assert!(page.is_empty());
    assert!(page.images.is_empty());
    assert_eq!(page.chars, 0);
    assert_eq!(page.words, 0);
}

#[test]
fn hidden_elements_are_skipped() {
    let html = r#"<html><body><article>
        <p>可见文字<span style="display: none">隐藏文字</span></p>
        <p style="color: red; display: none">整段隐藏</p>
        <p hidden>hidden 属性</p>
        <p>结尾</p>
    </article></body></html>"#;
    let page = parse_html("https://example.com/post", html);
    let markdown = &page.markdown;
    assert!(markdown.contains("可见文字"));
    assert!(markdown.contains("结尾"));
    assert!(!markdown.contains("隐藏文字"));
    assert!(!markdown.contains("整段隐藏"));
    assert!(!markdown.contains("hidden 属性"));
}

#[test]
fn lazy_loaded_images_fall_back_to_data_src() {
    let html = r#"<html><body><article>
        <p><img src="data:image/gif;base64,R0lGOD" data-src="/images/real.png" alt="懒加载"></p>
    </article></body></html>"#;
    let page = parse_html("https://example.com/post", html);
    assert_eq!(
        page.images,
        vec!["https://example.com/images/real.png".to_string()]
    );
    assert!(
        page.markdown
            .contains("https://example.com/images/real.png")
    );
}

#[test]
fn deeply_nested_markup_is_bounded_instead_of_overflowing_the_stack() {
    let mut html = String::from("<html><body><article>");
    for _ in 0..600 {
        html.push_str("<div>");
    }
    html.push_str("<p>deep text</p>");
    for _ in 0..600 {
        html.push_str("</div>");
    }
    html.push_str("</article></body></html>");

    // 深度超过上限的子树被跳过，但解析本身不能 panic / 栈溢出。
    let page = parse_html("https://example.com/deep", &html);
    assert!(!page.title.is_empty());
}

const WECHAT_FIXTURE: &str = r#"<!doctype html>
<html lang="zh-CN">
  <head>
    <title></title>
    <meta property="og:title" content="趋势：大模型服务能力内化">
    <meta property="og:site_name" content="微信公众平台">
  </head>
  <body>
    <div id="page-content" class="rich_media_area_primary">
      <div id="js_article" class="rich_media">
        <h1 class="rich_media_title">趋势：大模型服务能力内化</h1>
        <div id="js_content" class="rich_media_content">
          <section>
            <h2><span style="display: none;"><span leaf="">unset</span></span>从 CoT 到深度思考<span style="display:none"><span leaf="">unset</span></span></h2>
            <p>一些曾经的 Agent 设计模式，正逐渐内化到大模型服务之中。</p>
            <p hidden>整段隐藏的内容</p>
            <p><img class="rich_pages" data-src="https://mmbiz.qpic.cn/mmbiz_png/abc/640?wx_fmt=png"
                    src="data:image/svg+xml;base64,AAAA" alt="示意图"></p>
          </section>
        </div>
        <div class="rich_media_tool">
          <p>扫码关注公众号，获取更多内容。</p>
        </div>
      </div>
    </div>
  </body>
</html>"#;

#[test]
fn wechat_articles_use_js_content_as_the_body() {
    let page = parse_html("https://mp.weixin.qq.com/s/abc?scene=1", WECHAT_FIXTURE);

    assert_eq!(page.title, "趋势：大模型服务能力内化");
    assert_eq!(page.site, "微信公众平台");
    assert_eq!(page.lang, "zh-CN");

    let markdown = &page.markdown;
    assert!(markdown.contains("Agent 设计模式"), "{markdown}");
    assert!(markdown.contains("深度思考"), "{markdown}");
    assert!(
        !markdown.contains("扫码关注公众号"),
        "正文容器外的二维码/关注提示不应进入正文：{markdown}"
    );
    assert!(
        !markdown.contains("unset"),
        "微信小标题两侧的 display:none 标记不能进入正文：{markdown}"
    );
    assert!(
        !markdown.contains("整段隐藏"),
        "hidden 属性标记的段落应跳过：{markdown}"
    );

    // `src` 是 data: 占位图时必须回退到 `data-src`。
    assert_eq!(
        page.images,
        vec!["https://mmbiz.qpic.cn/mmbiz_png/abc/640?wx_fmt=png".to_string()]
    );
    assert!(markdown.contains("https://mmbiz.qpic.cn/mmbiz_png/abc/640?wx_fmt=png"));
}

#[test]
fn challenge_pages_are_recognised_instead_of_saved_as_articles() {
    // 微信验证页：正文很薄 + 页面文字命中。
    let detail = tela_content::webpage::challenge_detail(
        "https://mp.weixin.qq.com/mp/wappoc_appmsgcaptcha?poc_token=abc",
        "<html><body>环境异常 当前环境异常，完成验证后即可继续访问。</body></html>",
        30,
    )
    .expect("应判定为验证页");
    assert!(detail.contains("微信"), "{detail}");

    // 通用拦截页（Cloudflare 等）。
    let detail = tela_content::webpage::challenge_detail(
        "https://example.com/protected",
        "<html><title>Just a moment...</title><body>Enable JavaScript and cookies to continue</body></html>",
        20,
    );
    assert!(detail.is_some());

    // 长文正好引用了「环境异常」：不能误判。
    assert!(
        tela_content::webpage::challenge_detail(
            "https://example.com/post",
            "<html><body><p>环境异常时怎么办？</p></body></html>",
            3_000,
        )
        .is_none(),
        "正文足够长时不应按验证页处理"
    );

    // 普通短页面没有验证特征：也不算拦截。
    assert!(
        tela_content::webpage::challenge_detail(
            "https://example.com/post",
            "<html><body><p>一句短话</p></body></html>",
            10,
        )
        .is_none()
    );
}

#[test]
fn blocked_pages_produce_a_clear_error_for_the_dialog() {
    let error = tela_proto::TelaError::Blocked {
        url: "https://mp.weixin.qq.com/s/abc".to_string(),
        detail: "微信公众平台返回的是「环境异常」验证页，没有返回文章内容".to_string(),
    };
    let text = error.to_string();
    assert!(text.contains("mp.weixin.qq.com"), "{text}");
    assert!(text.contains("环境异常"), "{text}");
    assert!(text.contains("完成验证后再收藏"), "{text}");
    assert_eq!(error.code(), "blocked_page");
    assert_eq!(error.exit_code(), 21, "需要用户去浏览器处理");
    assert!(!error.is_retryable());
}

/// 真实网络冒烟：默认忽略，手动运行验证「抓取 → 抽取 → Markdown」整条链路。
///
/// ```bash
/// cargo test -p tela-content --test webpage -- --ignored
/// ```
#[test]
#[ignore = "需要网络：手动运行验证真实抓取"]
fn fetches_a_real_page_over_the_network() {
    let page = tela_content::webpage::fetch("https://example.com").expect("抓取 example.com");
    assert_eq!(page.url, "https://example.com/");
    assert!(
        page.title.to_ascii_lowercase().contains("example"),
        "{page:?}"
    );
    assert!(!page.is_empty(), "应该抽到正文：{page:?}");
    assert!(page.chars > 0);
    assert_eq!(page.lang, "en");
}

/// 微信文章：桌面 UA 会被重定向到验证页，必须用微信内 UA 才拿得到正文。
#[test]
#[ignore = "需要网络：手动验证微信文章抓取"]
fn fetches_a_wechat_article_with_the_weixin_user_agent() {
    let page =
        tela_content::webpage::fetch("https://mp.weixin.qq.com/s/F0PJHTfbn-Jt1kvHmWZ-vg?scene=1")
            .expect("抓取公众号文章");
    assert!(!page.is_empty(), "应抽到正文：{page:?}");
    assert!(!page.title.is_empty());
    assert!(
        !page.markdown.contains("unset"),
        "微信小标题的 unset 标记必须剔除：{}",
        page.markdown
    );
    assert!(
        page.chars > 500,
        "公众号文章正文不应只有几个字（chars={}）：{page:?}",
        page.chars
    );
    eprintln!(
        "wechat: title={:?} site={:?} chars={} markdown_len={} images={}",
        page.title,
        page.site,
        page.chars,
        page.markdown.len(),
        page.images.len()
    );
}

/// X（Twitter）帖子：标题取正文第一行，正文是长推文全文，而不是导航/作者名。
#[test]
#[ignore = "需要网络：手动验证 X 帖子抓取"]
fn fetches_an_x_post_with_a_smart_title() {
    let page = tela_content::webpage::fetch("https://x.com/UPing123zzz/status/2116729183334523014")
        .expect("抓取 X 帖子");
    eprintln!(
        "x: title={:?} site={:?} chars={} markdown_len={} images={}",
        page.title,
        page.site,
        page.chars,
        page.markdown.len(),
        page.images.len()
    );
    assert_eq!(page.site, "X");
    assert!(
        page.title.starts_with("我现在越来越觉得"),
        "标题应是正文第一行，而不是作者名：{:?}",
        page.title
    );
    assert!(
        page.title.chars().count() <= 81,
        "标题不应是整篇正文：{:?}",
        page.title
    );
    assert!(
        page.chars > 1_000,
        "长推文应抓到全文（不是 og:description 的截断版），实际 {} 字",
        page.chars
    );
    assert!(page.markdown.contains("Ethan 恩宇"), "正文应包含作者");
    assert!(!page.markdown.contains("Sign up"), "不应混入登录按钮");
}

/// X 长文章：标题用文章标题，正文来自 SSR 的 Draft.js 块（而不是「作者 on X」）。
#[test]
#[ignore = "需要网络：手动验证 X 文章抓取"]
fn fetches_an_x_article_with_its_own_title() {
    let page = tela_content::webpage::fetch("https://x.com/SuJinyan6/article/2116730566678868190")
        .expect("抓取 X 文章");
    eprintln!(
        "x article: title={:?} site={:?} chars={} markdown_len={} images={}",
        page.title,
        page.site,
        page.chars,
        page.markdown.len(),
        page.images.len()
    );
    assert_eq!(page.site, "X");
    assert_eq!(page.title, "2026 北美普通 PhD 找工小结");
    assert!(page.chars > 5_000, "文章应抓到全文，实际 {} 字", page.chars);
    assert!(page.images.len() >= 5, "文章正文里的图片应保留");
    assert!(page.markdown.contains("Jinyan Su"), "正文应包含作者");
    assert!(
        page.markdown.contains("## "),
        "小标题应保留为 Markdown 标题"
    );
    assert!(!page.markdown.contains("Sign up"), "不应混入登录按钮");
}

/// 同一篇文章的帖子地址（`/user/status/<id>`）也应走文章结构，标题不用「作者 on X」。
#[test]
#[ignore = "需要网络：手动验证 X 文章帖子地址抓取"]
fn fetches_an_x_article_from_its_status_url() {
    let page = tela_content::webpage::fetch("https://x.com/SuJinyan6/status/2116730566678868190")
        .expect("抓取 X 文章帖子");
    assert_eq!(page.site, "X");
    assert_eq!(page.title, "2026 北美普通 PhD 找工小结");
    assert!(page.chars > 5_000, "实际 {} 字", page.chars);
    assert!(page.images.len() >= 5, "文章正文里的图片应保留");
    assert!(!page.title.contains(" on X"), "标题不应是作者/站点外壳");
}
