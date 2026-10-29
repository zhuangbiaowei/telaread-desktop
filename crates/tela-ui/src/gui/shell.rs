//! 三栏外壳的 GPUI 视图（S2）。
//!
//! 只做“把 [`ShellViewModel`] 画成元素 + 把点击转成模型操作”，
//! 不查库、不发 HTTP、不解析正文。阅读面板在 S3 由 [`crate::gui::reader`] 接管。

use std::sync::Arc;

use tela_content::api::ContentApi;
use tela_proto::TelaError;

use crate::account::{AccountMode, AccountService, AccountStatus, LoginPhase, LoginTicket};
use crate::gui::reader::ReaderView;
use crate::shell::{ArticleItem, FeedItem, ShellData, ShellViewModel};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const BG: u32 = 0x11111b;
const SIDEBAR: u32 = 0x181825;
const PANEL: u32 = 0x1e1e2e;
const BORDER: u32 = 0x313244;
const TEXT: u32 = 0xcdd6f4;
const MUTED: u32 = 0x6c7086;
const SUBTLE: u32 = 0xa6adc8;
const ACCENT: u32 = 0x89b4fa;
const DANGER: u32 = 0xf38ba8;
const WARN: u32 = 0xf9e2af;

/// 三栏外壳视图：左订阅、中文章、右阅读面板。
pub struct ShellView {
    data: Arc<dyn ShellData>,
    content: Arc<dyn ContentApi>,
    model: ShellViewModel,
    reader: Entity<ReaderView>,
    accounts: Option<Arc<dyn AccountService>>,
    account: AccountStatus,
    login: LoginPhase,
    account_notice: Option<String>,
    login_generation: u64,
    login_task: Option<Task<()>>,
    sync_in_flight: bool,
    sync_again: bool,
    sync_task: Option<Task<()>>,
}

impl ShellView {
    /// 装配视图并做首次加载（同步；mock 数据在微秒级，真实库也是本地 SQLite）。
    pub fn new(
        data: Arc<dyn ShellData>,
        content: Arc<dyn ContentApi>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_accounts(data, content, None, cx)
    }

    /// 带 Login/Logout 的装配。
    pub fn new_with_accounts(
        data: Arc<dyn ShellData>,
        content: Arc<dyn ContentApi>,
        accounts: Option<Arc<dyn AccountService>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let reader = cx.new(|cx| ReaderView::new(data.clone(), content.clone(), cx));
        let mut model = ShellViewModel::new();
        model.reload(data.as_ref());
        let account = accounts
            .as_ref()
            .map(|accounts| accounts.status())
            .unwrap_or_default();
        let mut view = Self {
            data,
            content,
            model,
            reader,
            accounts,
            account,
            login: LoginPhase::Idle,
            account_notice: None,
            login_generation: 0,
            login_task: None,
            sync_in_flight: false,
            sync_again: false,
            sync_task: None,
        };
        view.sync_reader(cx);
        view
    }

    /// 当前视图模型（测试与状态栏用）。
    pub fn model(&self) -> &ShellViewModel {
        &self.model
    }

    /// 阅读面板实体（测试/后续状态联动用）。
    pub fn reader(&self) -> Entity<ReaderView> {
        self.reader.clone()
    }

    /// 当前账号快照。
    pub fn account(&self) -> &AccountStatus {
        &self.account
    }

    /// 当前登录阶段。
    pub fn login_phase(&self) -> &LoginPhase {
        &self.login
    }

    /// 内容 API 句柄（诊断/后续设置页用）。
    pub fn content(&self) -> Arc<dyn ContentApi> {
        Arc::clone(&self.content)
    }

    /// 选中订阅；返回是否发生变化。
    pub fn select_feed(&mut self, feed_id: i64, cx: &mut Context<Self>) -> bool {
        let changed = self.model.select_feed(feed_id, self.data.as_ref());
        if changed {
            self.sync_reader(cx);
            // 切订阅会显示该订阅的第一篇，视同打开。
            if let Some(article_id) = self.model.selected_article {
                let _ = self.mark_read_if_needed(article_id, cx);
            }
        }
        changed
    }

    /// 选中文章；返回是否发生变化。
    pub fn select_article(&mut self, article_id: i64, cx: &mut Context<Self>) -> bool {
        let changed = self.model.select_article(article_id);
        if changed {
            self.sync_reader(cx);
        }
        // 即使这篇已经选中（例如启动时自动展示的第一篇），再点一次也应当标已读。
        let marked = self.mark_read_if_needed(article_id, cx);
        changed || marked
    }

    /// 重新从数据源刷新（后台同步完成后调用）。
    pub fn reload(&mut self, cx: &mut Context<Self>) -> bool {
        let changed = self.model.reload(self.data.as_ref());
        if changed {
            self.sync_reader(cx);
        }
        changed
    }

    /// 把当前选中的文章交给阅读面板；同一篇重复调用由阅读面板自己忽略。
    fn sync_reader(&mut self, cx: &mut Context<Self>) {
        if let Some(article) = self.model.selected_article_item().cloned() {
            self.reader.update(cx, |reader, cx| {
                reader.select(article.id, article.title, cx)
            });
        }
    }

    /// 打开文章即已读（尊重 `reader.mark_on_open`）：本地先变，再后台推送。
    fn mark_read_if_needed(&mut self, article_id: i64, cx: &mut Context<Self>) -> bool {
        if self.account.mode != AccountMode::Real {
            return false;
        }
        // read 令牌无法写：不要每点一篇文章弹一次 403，等用户重新授权为 write。
        if self.account.scope.as_deref() != Some("write") {
            return false;
        }
        let Some(accounts) = self.accounts.clone() else {
            return false;
        };
        match accounts.mark_read(article_id) {
            Ok(true) => {
                // 立刻刷列表：未读计数/加粗在当前帧就消失，不等网络。
                self.reload(cx);
                self.schedule_sync(cx);
                true
            }
            Ok(false) => false,
            Err(error) => {
                self.account_notice = Some(format!("标记已读失败：{error}"));
                cx.notify();
                false
            }
        }
    }

    /// 合并短时间内的多次同步请求：一次在飞时只记一个“再来一轮”。
    fn schedule_sync(&mut self, cx: &mut Context<Self>) {
        let Some(accounts) = self.accounts.clone() else {
            return;
        };
        if self.sync_in_flight {
            self.sync_again = true;
            return;
        }
        self.sync_in_flight = true;
        let generation = self.login_generation;
        let task = cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = cx
                .background_executor()
                .spawn(async move { accounts.sync_now() })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.login_generation != generation {
                    return;
                }
                view.sync_in_flight = false;
                if let Err(error) = result {
                    view.account_notice = Some(format!("已读状态已入队，但同步失败：{error}"));
                }
                view.reload(cx);
                if view.sync_again {
                    view.sync_again = false;
                    view.schedule_sync(cx);
                }
                cx.notify();
            });
        });
        self.sync_task = Some(task);
    }

    /// 侧栏底部的账号面板：登录 / 等待授权 / 退出。
    fn account_panel(&self, cx: &mut Context<Self>) -> Div {
        let mut panel = v_flex().gap_2().child(
            div()
                .text_xs()
                .text_color(rgb(SUBTLE))
                .child(self.account.label()),
        );

        if self.account.mode == AccountMode::Real && !self.account.client_id_configured {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(rgb(WARN))
                    .child("缺少 TELA_CLIENT_ID：用 scripts/tela-gui-real.sh 启动"),
            );
        }
        if let Some(error) = &self.account.store_error {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(rgb(DANGER))
                    .child(format!("凭据库：{error}")),
            );
        }

        match &self.login {
            LoginPhase::Idle => {
                if self.account.can_logout() {
                    panel = panel.child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .truncate()
                                    .child(format!("库 {}", self.account.db_path)),
                            )
                            .child(
                                div()
                                    .id("account-logout")
                                    .role(Role::Button)
                                    .aria_label("退出登录")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .border_1()
                                    .border_color(rgb(DANGER))
                                    .text_xs()
                                    .text_color(rgb(DANGER))
                                    .hover(|style| style.bg(rgb(DANGER)).text_color(rgb(BG)))
                                    .on_click(cx.listener(|this, _, _, cx| this.logout(cx)))
                                    .child("退出"),
                            ),
                    );
                } else if self.account.mode == AccountMode::Real {
                    panel = panel.child(
                        div()
                            .id("account-login")
                            .role(Role::Button)
                            .aria_label("登录 Tela")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(ACCENT))
                            .text_color(rgb(BG))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .on_click(cx.listener(|this, _, _, cx| this.start_login(cx)))
                            .child("登录 Tela"),
                    );
                }
            }
            LoginPhase::Waiting(ticket) => {
                panel = panel
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(ACCENT))
                            .child("等待浏览器授权…"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .truncate()
                            .child(ticket.auth_url.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .id("account-open-auth")
                                    .role(Role::Button)
                                    .aria_label("打开授权页")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(rgb(ACCENT))
                                    .text_color(rgb(BG))
                                    .text_xs()
                                    .on_click(cx.listener(|this, _, _, cx| this.open_auth_page(cx)))
                                    .child("打开"),
                            )
                            .child(
                                div()
                                    .id("account-copy-auth")
                                    .role(Role::Button)
                                    .aria_label("复制授权链接")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .text_xs()
                                    .text_color(rgb(SUBTLE))
                                    .on_click(cx.listener(|this, _, _, cx| this.copy_auth_link(cx)))
                                    .child("复制链接"),
                            )
                            .child(
                                div()
                                    .id("account-cancel-login")
                                    .role(Role::Button)
                                    .aria_label("取消登录")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_login(cx)))
                                    .child("取消"),
                            ),
                    );
            }
            LoginPhase::Error(error) => {
                panel = panel
                    .child(div().text_xs().text_color(rgb(DANGER)).child(error.clone()))
                    .child(
                        div()
                            .id("account-retry-login")
                            .role(Role::Button)
                            .aria_label("重试登录")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(ACCENT))
                            .text_color(rgb(BG))
                            .text_xs()
                            .on_click(cx.listener(|this, _, _, cx| this.start_login(cx)))
                            .child("重试登录"),
                    );
            }
        }

        if let Some(notice) = &self.account_notice {
            panel = panel.child(div().text_xs().text_color(rgb(MUTED)).child(notice.clone()));
        }
        panel
    }

    /// 点击「登录」：开端口、拿 URL、尝试开浏览器，并把等待放到后台。
    pub fn start_login(&mut self, cx: &mut Context<Self>) {
        let Some(accounts) = self.accounts.clone() else {
            self.login = LoginPhase::Error("当前是 demo 装配，没有账号能力".into());
            self.account_notice = None;
            cx.notify();
            return;
        };
        self.login_generation = self.login_generation.wrapping_add(1);
        let generation = self.login_generation;
        match accounts.begin_login() {
            Ok(ticket) => {
                // 终端里也打印一份：浏览器打不开时用户可直接复制。
                eprintln!("tela-gui: 授权地址 {}", ticket.auth_url);
                let open_error = accounts.open_auth_url(&ticket).err();
                self.account_notice = Some(match open_error {
                    Some(error) => {
                        format!("浏览器未打开：{error}；链接已打印到终端，可点「复制链接」")
                    }
                    None => "已在浏览器打开授权页，等待授权…".to_string(),
                });
                self.login = LoginPhase::Waiting(ticket.clone());
                self.spawn_login(accounts, ticket, generation, cx);
            }
            Err(error) => {
                self.login = LoginPhase::Error(error.to_string());
                self.account_notice = None;
            }
        }
        cx.notify();
    }

    fn spawn_login(
        &mut self,
        accounts: Arc<dyn AccountService>,
        ticket: LoginTicket,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result: Result<(AccountStatus, Result<(), TelaError>), TelaError> = cx
                .background_executor()
                .spawn(async move {
                    let status = accounts.wait_login(&ticket)?;
                    let sync = accounts.sync_now();
                    Ok((status, sync))
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                if view.login_generation != generation {
                    return;
                }
                match result {
                    Ok((status, sync)) => {
                        view.account = status;
                        view.login = LoginPhase::Idle;
                        view.account_notice = sync
                            .err()
                            .map(|error| format!("登录成功，但同步失败：{error}"));
                        view.reload(cx);
                    }
                    Err(error) => {
                        view.login = LoginPhase::Error(error.to_string());
                        view.account_notice = None;
                    }
                }
                cx.notify();
            });
        });
        self.login_task = Some(task);
    }

    /// 重新打开授权页（浏览器第一次没起来时）。
    pub fn open_auth_page(&mut self, cx: &mut Context<Self>) {
        let Some(ticket) = self.login.ticket().cloned() else {
            return;
        };
        let Some(accounts) = self.accounts.clone() else {
            return;
        };
        match accounts.open_auth_url(&ticket) {
            Ok(()) => self.account_notice = Some("已请求打开浏览器".to_string()),
            Err(error) => {
                self.account_notice = Some(format!("仍无法打开浏览器：{error}；可点「复制链接」"))
            }
        }
        cx.notify();
    }

    /// 把授权链接放进剪贴板。
    pub fn copy_auth_link(&mut self, cx: &mut Context<Self>) {
        let Some(ticket) = self.login.ticket().cloned() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(ticket.auth_url));
        self.account_notice = Some("授权链接已复制到剪贴板".to_string());
        cx.notify();
    }

    /// 取消登录：关闭回环端口，并让迟到的后台结果失效。
    pub fn cancel_login(&mut self, cx: &mut Context<Self>) {
        if let Some(accounts) = self.accounts.clone() {
            accounts.cancel_login();
        }
        self.login_generation = self.login_generation.wrapping_add(1);
        self.login = LoginPhase::Idle;
        self.account_notice = Some("已取消登录".to_string());
        cx.notify();
    }

    /// 退出登录：撤销令牌、清本地账号数据、刷新列表。
    pub fn logout(&mut self, cx: &mut Context<Self>) {
        let Some(accounts) = self.accounts.clone() else {
            return;
        };
        self.login_generation = self.login_generation.wrapping_add(1);
        self.login = LoginPhase::Idle;
        self.sync_in_flight = false;
        self.sync_again = false;
        self.sync_task = None;
        match accounts.logout() {
            Ok(status) => {
                self.account = status;
                self.account_notice = Some("已退出登录".to_string());
                self.reload(cx);
            }
            Err(error) => {
                // 本地凭据通常已经被清掉，状态以服务端/本地真实值为准。
                self.account = accounts.status();
                self.account_notice = Some(format!("退出时出错：{error}"));
            }
        }
        cx.notify();
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        v_flex()
            .w(px(260.))
            .h_full()
            .bg(rgb(SIDEBAR))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                v_flex()
                    .p_4()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Tela"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child("本地优先阅读器"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(SUBTLE))
                            .child(model.status_line()),
                    ),
            )
            .child(Self::feed_list(model, cx))
            .child(
                v_flex()
                    .p_3()
                    .gap_1()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                        "{} 个订阅 · {} 篇未读",
                        model.feeds.len(),
                        model.unread_total()
                    )))
                    .child(self.account_panel(cx)),
            )
    }

    fn feed_list(model: &ShellViewModel, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = v_flex()
            .id("feed-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_2()
            .gap_1();

        if let Some(error) = &model.feeds_error {
            return list.child(Self::inline_error("订阅列表加载失败", error));
        }
        if model.feeds.is_empty() {
            return list.child(Self::sidebar_empty());
        }

        for feed in &model.feeds {
            list = list.child(Self::feed_row(feed, model.selected_feed, cx));
        }
        list
    }

    fn feed_row(
        feed: &FeedItem,
        selected: Option<i64>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let feed_id = feed.id;
        let is_selected = selected == Some(feed.id);
        v_flex()
            .id(("feed", feed.id as u64))
            .role(Role::Button)
            .aria_label(feed.title.clone())
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .when(is_selected, |element| element.bg(rgb(BORDER)))
            .when(!is_selected, |element| {
                element.hover(|style| style.bg(rgb(PANEL)))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.select_feed(feed_id, cx) {
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(if is_selected {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .truncate()
                            .child(feed.title.clone()),
                    )
                    .when(feed.unread > 0, |element| {
                        element.child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(ACCENT))
                                .child(feed.unread.to_string()),
                        )
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .truncate()
                    .child(feed.feed_url.clone()),
            )
    }

    fn sidebar_empty() -> Div {
        v_flex()
            .p_4()
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child("还没有订阅"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("同步接入后，这里会出现订阅与文章。"),
            )
    }

    fn main_panel(&self, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        if let Some(error) = &model.feeds_error {
            return h_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(rgb(BG))
                .child(Self::full_page_error("无法加载订阅", error));
        }
        if model.is_empty() {
            return h_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(rgb(BG))
                .child(Self::full_page_empty());
        }

        h_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(rgb(BG))
            .child(self.article_pane(cx))
            .child(self.reader_pane())
    }

    fn article_pane(&self, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        let title = model
            .selected_feed_item()
            .map(|feed| feed.title.clone())
            .unwrap_or_else(|| "文章".to_string());
        let count = model.articles.len();

        let mut list = v_flex()
            .id("article-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_2()
            .gap_1();

        if let Some(error) = &model.articles_error {
            list = list.child(Self::inline_error("文章列表加载失败", error));
        } else if model.articles.is_empty() {
            list = list.child(
                v_flex()
                    .p_4()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().child("这个订阅还没有文章"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("同步完成后，文章会出现在这里。"),
                    ),
            );
        } else {
            for article in &model.articles {
                list = list.child(Self::article_row(article, model.selected_article, cx));
            }
        }

        v_flex()
            .w(px(340.))
            .h_full()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                h_flex()
                    .p_4()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("{count} 篇")),
                    ),
            )
            .child(list)
    }

    fn article_row(
        article: &ArticleItem,
        selected: Option<i64>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let article_id = article.id;
        let is_selected = selected == Some(article.id);
        v_flex()
            .id(("article", article.id as u64))
            .role(Role::Button)
            .aria_label(article.title.clone())
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .when(is_selected, |element| element.bg(rgb(BORDER)))
            .when(!is_selected, |element| {
                element.hover(|style| style.bg(rgb(SIDEBAR)))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.select_article(article_id, cx) {
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .when(article.unread, |element| {
                                element.font_weight(FontWeight::SEMIBOLD)
                            })
                            .truncate()
                            .child(article.title.clone()),
                    )
                    .when(article.unread, |element| {
                        element.child(div().text_xs().text_color(rgb(ACCENT)).child("未读"))
                    }),
            )
            .when_some(article.excerpt.clone(), |element, excerpt| {
                element.child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .truncate()
                        .child(excerpt),
                )
            })
    }

    fn reader_pane(&self) -> Div {
        // 阅读面板是独立实体：它自己管理异步加载/失败重试，外壳只负责选文章。
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.reader.clone())
    }

    fn inline_error(title: &str, detail: &str) -> Div {
        v_flex()
            .p_3()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(0xf38ba8))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xf38ba8))
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(SUBTLE))
                    .child(detail.to_string()),
            )
    }

    fn full_page_error(title: &str, detail: &str) -> Div {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .p_8()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xf38ba8))
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(SUBTLE))
                    .child(detail.to_string()),
            )
    }

    fn full_page_empty() -> Div {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .p_8()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("还没有订阅"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("这是 S2 的空态；接入同步后，这里会出现订阅列表。"),
            )
    }
}

impl Render for ShellView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.sidebar(cx);
        let main = self.main_panel(cx);
        h_flex()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .child(sidebar)
            .child(main)
    }
}
