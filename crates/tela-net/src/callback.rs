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

        let mut stream = loop {
            match self.listener.accept() {
                Ok((stream, _)) => break stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
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
        };

        let request = read_request(&mut stream)?;
        let answer = self.validate(&request);
        respond(&mut stream, answer.is_ok());
        answer
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
fn read_request(stream: &mut TcpStream) -> Result<String, TelaError> {
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
    text.lines()
        .next()
        .map(str::to_string)
        .ok_or_else(|| TelaError::Auth {
            ctx: tela_proto::WireContext::new("http://127.0.0.1", "connect"),
            detail: "浏览器回调为空".into(),
        })
}

/// 无论成功失败都给浏览器一个可读的短页面。
fn respond(stream: &mut TcpStream, ok: bool) {
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
