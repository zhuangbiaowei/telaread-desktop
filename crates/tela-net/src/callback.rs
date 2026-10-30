//! 回环回调监听（RFC 8252 §7.3）：`http://127.0.0.1:<随机端口>/callback`。
//!
//! 端口在运行时才确定，注册时只登记「无端口」的回环地址；服务端在任意端口上匹配
//! 同一 host/path/query。`localhost` 永远不会被接受，因为可能解析到非回环地址。

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use tela_proto::{ISSUER, TelaError};

use crate::oauth::{AuthConfig, Pkce, Tokens, exchange_code, new_state};
use crate::token_store::{StoredCredential, TokenStore};

/// 浏览器回来后的一次成功回调。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackAnswer {
    /// 授权码（两分钟、单次有效）。
    pub code: String,
}

/// 一次已经「开好端口、拼好 URL」的登录会话。
#[derive(Debug)]
pub struct PendingLogin {
    config: AuthConfig,
    listener: TcpListener,
    pkce: Pkce,
    state: String,
    redirect_uri: String,
    auth_url: String,
}

impl PendingLogin {
    /// 监听回环随机端口并拼出授权 URL（此时还没有打开浏览器）。
    pub fn begin(config: &AuthConfig) -> Result<Self, TelaError> {
        config.validate()?;
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|err| TelaError::Config {
                detail: format!("无法监听 127.0.0.1 的随机端口：{err}"),
            })?;
        let port = listener
            .local_addr()
            .map_err(|err| TelaError::Config {
                detail: format!("无法读取监听端口：{err}"),
            })?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}{}", config.redirect_path);
        let pkce = Pkce::generate();
        let state = new_state();
        let auth_url =
            crate::oauth::authorization_url(config, &redirect_uri, &state, &pkce.challenge);

        Ok(Self {
            config: config.clone(),
            listener,
            pkce,
            state,
            redirect_uri,
            auth_url,
        })
    }

    /// 要交给系统浏览器的授权 URL。
    pub fn auth_url(&self) -> &str {
        &self.auth_url
    }

    /// 本次使用的回环 redirect_uri（换令牌时要原样再送一次）。
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// 阻塞等待浏览器回调，校验 `state` 与 `iss`。
    pub fn wait(&self, timeout: Duration) -> Result<CallbackAnswer, TelaError> {
        let deadline = Instant::now() + timeout;
        self.listener
            .set_nonblocking(true)
            .map_err(|err| TelaError::Config {
                detail: format!("无法设置监听超时：{err}"),
            })?;

        loop {
            let mut stream = self.accept_before(deadline)?;
            // Windows 的 accept 让新 socket 继承监听 socket 的属性（MSDN：新 socket
            // "has the same properties as socket s"）。listener 是非阻塞的，浏览器
            // 还没把请求写进来时 read 会立刻 WouldBlock——必须显式改回阻塞，否则
            // 回调被误判成空请求、端口随即关闭，浏览器看到的就是「127.0.0.1 拒绝连接」。
            if stream.set_nonblocking(false).is_err() {
                continue;
            }
            match read_request(&mut stream) {
                // 只有「GET + 回调路径」才是真正的授权回调，交给 validate 严格校验
                // （state/iss/code 错了要报给用户）。
                Ok(Some(request)) if self.looks_like_callback(&request) => {
                    let answer = self.validate(&request);
                    respond(&mut stream, answer.is_ok());
                    return answer;
                }
                // 浏览器预连接、安全软件探活、TLS 探测、favicon 等：忽略它们，
                // 继续等真正的回调，不要因此结束登录。
                Ok(_) | Err(_) => continue,
            }
        }
    }

    /// 请求行是否是本次登录的回调（GET + 配置的回调路径）。
    fn looks_like_callback(&self, request_line: &str) -> bool {
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let Some(target) = parts.next() else {
            return false;
        };
        method == "GET"
            && reqwest::Url::parse(&format!("http://127.0.0.1{target}"))
                .is_ok_and(|url| url.path() == self.config.redirect_path)
    }

    /// 在 deadline 之前等下一个连接；超时返回错误。
    fn accept_before(&self, deadline: Instant) -> Result<TcpStream, TelaError> {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => return Ok(stream),
                // WouldBlock：还没人来。ConnectionReset/Aborted：Windows 上对端在
                // accept 之前就断开（探活/预连接）会得到 WSAECONNRESET，不是监听
                // 坏了，跳过继续等。两种情况都只睡眠后重试。
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                    ) =>
                {
                    if Instant::now() >= deadline {
                        return Err(TelaError::Auth {
                            ctx: tela_proto::WireContext::new(self.auth_url.clone(), "connect"),
                            detail: "等待浏览器回调超时；请重试并完成授权".into(),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(err) => {
                    return Err(TelaError::Config {
                        detail: format!("回环回调监听失败：{err}"),
                    });
                }
            }
        }
    }

    /// 校验请求行里的 query；`error` / `state` / `iss` / `code` 一个都不能错。
    fn validate(&self, request_line: &str) -> Result<CallbackAnswer, TelaError> {
        let auth_error = |detail: String| TelaError::Auth {
            ctx: tela_proto::WireContext::new(self.auth_url.clone(), "connect"),
            detail,
        };

        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let target = parts.next().unwrap_or_default();
        if method != "GET" {
            return Err(auth_error(format!("回调不是 GET（收到 {method}）")));
        }

        let base = format!("http://127.0.0.1{}", self.config.redirect_path);
        let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}"))
            .map_err(|err| auth_error(format!("回调地址无法解析：{err}")))?;
        let expected = reqwest::Url::parse(&base).expect("合法常量 URL");
        if url.path() != expected.path() {
            return Err(auth_error(format!(
                "回调路径不是 {}（收到 {}）",
                expected.path(),
                url.path()
            )));
        }

        let mut state = None;
        let mut code = None;
        let mut iss = None;
        let mut error = None;
        let mut error_description = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "state" => state = Some(value.into_owned()),
                "code" => code = Some(value.into_owned()),
                "iss" => iss = Some(value.into_owned()),
                "error" => error = Some(value.into_owned()),
                "error_description" => error_description = Some(value.into_owned()),
                _ => {}
            }
        }

        if state.as_deref() != Some(self.state.as_str()) {
            return Err(auth_error("state 不匹配：可能不是这次授权，已丢弃".into()));
        }
        if iss.as_deref() != Some(ISSUER) {
            return Err(auth_error(format!(
                "iss 不是 {ISSUER}（收到 {:?}），已丢弃",
                iss.as_deref().unwrap_or("(缺失)")
            )));
        }

        if let Some(error) = error {
            let detail = match error_description {
                Some(description) if !description.is_empty() => format!("{error}：{description}"),
                _ => error,
            };
            return Err(auth_error(detail));
        }

        code.filter(|code| !code.is_empty())
            .map(|code| CallbackAnswer { code })
            .ok_or_else(|| auth_error("回调没有 code".into()))
    }

    /// 用授权码换令牌，并把刷新令牌交给存储。
    ///
    /// 返回的 `Tokens` 只应留在内存；刷新令牌已按文档要求写进 [`TokenStore`]。
    pub fn complete(
        self,
        answer: &CallbackAnswer,
        store: &dyn TokenStore,
        account: &str,
    ) -> Result<Tokens, TelaError> {
        let tokens = exchange_code(
            &self.config,
            &answer.code,
            &self.redirect_uri,
            &self.pkce.verifier,
        )?;
        store.save(
            account,
            &StoredCredential {
                refresh_token: tokens.refresh_token.clone(),
                scope: tokens.scope.clone(),
                handle: None,
            },
        )?;
        Ok(tokens)
    }
}

/// 读一次 HTTP 请求，返回请求行（第一行）。只读到头部结束，够用即可。
///
/// 没有收到任何数据（预连接 / 探活 / 对端直接断开）时返回 `Ok(None)`，
/// 由调用方继续等下一个连接，而不是把整次登录判死。
fn read_request(stream: &mut TcpStream) -> Result<Option<String>, TelaError> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buffer = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.windows(4).any(|window| window == b"\r\n\r\n") || buffer.len() > 16 * 1024
                {
                    break;
                }
            }
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(err) => {
                return Err(TelaError::Config {
                    detail: format!("读取浏览器回调失败：{err}"),
                });
            }
        }
    }
    let text = String::from_utf8_lossy(&buffer);
    Ok(text
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string))
}

/// 无论成功失败都给浏览器一个可读的短页面。
fn respond(stream: &mut TcpStream, ok: bool) {
    // 浏览器已经断开时别把等待线程卡死。
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let (title, body) = if ok {
        ("Connected", "You can go back to Tela Desktop now.")
    } else {
        (
            "Not connected",
            "Tela Desktop could not finish connecting. Go back and try again.",
        )
    };
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{title}</title>\
         <body style=\"font-family:system-ui;margin:4rem auto;max-width:32rem\">\
         <h1>{title}</h1><p>{body}</p></body>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn state_of(auth_url: &str) -> String {
        reqwest::Url::parse(auth_url)
            .expect("合法授权 URL")
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .expect("授权 URL 必须带 state")
    }

    #[test]
    fn a_probe_connection_does_not_cancel_the_callback() {
        let pending = PendingLogin::begin(&AuthConfig::new("tela_app_test")).expect("绑定回环端口");
        let state = state_of(pending.auth_url());
        let port = reqwest::Url::parse(pending.redirect_uri())
            .expect("合法 redirect_uri")
            .port()
            .expect("redirect_uri 必须带端口");

        let waiter = std::thread::spawn(move || pending.wait(Duration::from_secs(10)));

        // 浏览器预连接 / 安全软件探活：连上立刻断开，一个字节都不发。
        drop(TcpStream::connect(("127.0.0.1", port)).expect("连接回环端口"));

        // 另一个 GET，但路径不是回调：也不能把登录判死。
        let mut favicon = TcpStream::connect(("127.0.0.1", port)).expect("连接回环端口");
        write!(
            favicon,
            "GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        )
        .expect("写探活请求");
        favicon.flush().expect("flush");
        drop(favicon);

        // 真正的回调仍然要能完成（旧实现会在这里返回「浏览器回调为空」并关端口）。
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接回环端口");
        write!(
            stream,
            "GET /callback?state={state}&iss={ISSUER}&code=code-123 HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\r\n"
        )
        .expect("写回调请求");
        stream.flush().expect("flush");

        let answer = waiter
            .join()
            .expect("等待线程不应 panic")
            .expect("回调应成功");
        assert_eq!(answer.code, "code-123");
    }
}
