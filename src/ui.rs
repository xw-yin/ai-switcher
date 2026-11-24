use std::rc::Rc;
use std::sync::OnceLock;

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::antigravity;
use crate::codex;
use crate::models::{
    format_reset_countdown, format_rfc3339_countdown, AccountStore, AntigravityAccount,
    AntigravityIdeAccount, CodexAccount, Platform,
};
use crate::store::{
    export_store_to_path, import_accounts_from_path, load_store, save_store, ImportSummary,
};
use crate::tray::TrayController;

fn tokio_rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("create tokio runtime")
    })
}

const BG_WINDOW: u32 = 0x16171a;
const BG_SURFACE: u32 = 0x1f2126;
const BG_CARD: u32 = 0x24262d;
const BG_CARD_ACTIVE: u32 = 0x262c36;
const BORDER_SUBTLE: u32 = 0x32353f;
const BORDER_ACTIVE: u32 = 0x3b82f6;
const TEXT_PRIMARY: u32 = 0xf3f4f6;
const TEXT_SECONDARY: u32 = 0x9ca3af;
const TEXT_MUTED: u32 = 0x6b7280;
const ACCENT_BLUE: u32 = 0x3b82f6;
const ACCENT_GREEN: u32 = 0x22c55e;
const ACCENT_AMBER: u32 = 0xf59e0b;
const ACCENT_RED: u32 = 0xef4444;

fn quota_color(remaining_pct: f64) -> Rgba {
    if remaining_pct >= 50.0 {
        rgb(ACCENT_GREEN)
    } else if remaining_pct >= 20.0 {
        rgb(ACCENT_AMBER)
    } else {
        rgb(ACCENT_RED)
    }
}

pub fn refresh_store_quotas_blocking() -> Option<AccountStore> {
    let mut store = load_store().ok()?;
    let updated = tokio_rt().block_on(async move {
        if let Ok(client) = codex::create_http_client() {
            for account in &mut store.codex_accounts {
                match codex::fetch_usage(&client, &account.auth_json).await {
                    Ok((usage, refreshed_auth)) => {
                        if let Some(new_auth) = refreshed_auth {
                            account.auth_json = new_auth;
                        }
                        if usage.plan_type.is_some() {
                            account.plan_type = usage.plan_type.clone();
                        }
                        account.usage = Some(usage);
                        account.usage_error = None;
                    }
                    Err(err) => {
                        account.usage_error = Some(err);
                    }
                }
            }
        }
        if let Ok(live) = antigravity::fetch_live_status().await {
            let email = live.email.clone();
            let _ = antigravity::capture_current_account(&mut store, Some(live));
            if let Some(found) = store
                .antigravity_accounts
                .iter()
                .find(|a| a.email.eq_ignore_ascii_case(&email))
            {
                store.active_antigravity_id = Some(found.id.clone());
            }
        }
        if let Ok(live_ide) = antigravity::fetch_ide_live_status().await {
            let email = live_ide.email.clone();
            let _ = antigravity::capture_current_ide_account(&mut store, Some(live_ide));
            if let Some(found) = store
                .antigravity_ide_accounts
                .iter()
                .find(|a| a.email.eq_ignore_ascii_case(&email))
            {
                store.active_antigravity_ide_id = Some(found.id.clone());
            }
        }
        let _ = save_store(&store);
        store
    });
    Some(updated)
}

pub struct MainView {
    store: AccountStore,
    platform: Platform,
    busy: bool,
    status_message: String,
    tray: Option<Rc<TrayController>>,
}

impl MainView {
    pub fn new(tray: Option<Rc<TrayController>>, cx: &mut Context<Self>) -> Self {
        let mut store = load_store().unwrap_or_default();
        // 首次启动时若本地为空，自动探测当前已登录的 Codex 账号
        if store.codex_accounts.is_empty() {
            let _ = codex::capture_current_account(&mut store);
            let _ = save_store(&store);
        }

        let view = Self {
            store,
            platform: Platform::Codex,
            busy: false,
            status_message: "就绪".to_string(),
            tray,
        };
        view.update_tray_title();

        // 窗口打开后立即后台刷新一次额度
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |this, cx| {
                this.refresh_all_quotas(cx);
            });
        })
        .detach();

        view
    }

    fn update_tray_title(&self) {
        if let Some(ref tray) = self.tray {
            tray.update_from_store(&self.store);
        }
    }

    fn authorize_new_account(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let platform = self.platform;
        self.status_message = match platform {
            Platform::Codex => "已打开浏览器，请在浏览器中完成 OpenAI 登录授权...".to_string(),
            Platform::Antigravity => {
                "已备份当前账号并重启 Antigravity，请在应用中登录新账号...".to_string()
            }
            Platform::AntigravityIde => {
                "已备份当前账号并重启 Antigravity IDE，请在 IDE 中登录新账号...".to_string()
            }
        };
        cx.notify();

        let mut store_clone = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    tokio_rt().block_on(async move {
                        let msg = match platform {
                            Platform::Codex => {
                                let name = codex::oauth_login_and_save(&mut store_clone).await?;
                                format!("授权成功并已保存 Codex 账号: {name}")
                            }
                            Platform::Antigravity => {
                                let email =
                                    antigravity::login_new_account_and_save(&mut store_clone)
                                        .await?;
                                format!("授权成功并已保存 Antigravity 账号: {email}")
                            }
                            Platform::AntigravityIde => {
                                let email =
                                    antigravity::login_new_ide_account_and_save(&mut store_clone)
                                        .await?;
                                format!("授权成功并已保存 Antigravity IDE 账号: {email}")
                            }
                        };
                        Ok::<(AccountStore, String), String>((store_clone, msg))
                    })
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                cx.activate(true);
                match result {
                    Ok((updated_store, msg)) => {
                        this.store = updated_store;
                        this.status_message = msg;
                        this.update_tray_title();
                        this.refresh_all_quotas(cx);
                    }
                    Err(err) => {
                        this.status_message = format!("授权失败: {err}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn capture_current(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let platform = self.platform;
        self.status_message = match platform {
            Platform::Codex => "正在捕获当前 Codex 账号...".to_string(),
            Platform::Antigravity => "正在捕获当前 Antigravity 账号...".to_string(),
            Platform::AntigravityIde => "正在捕获当前 Antigravity IDE 账号...".to_string(),
        };
        cx.notify();

        let mut store_clone = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    tokio_rt().block_on(async move {
                        let msg = match platform {
                            Platform::Codex => {
                                let name = codex::capture_current_account(&mut store_clone)?;
                                save_store(&store_clone)?;
                                format!("已捕获 Codex 账号: {name}")
                            }
                            Platform::Antigravity => {
                                let live = antigravity::fetch_live_status().await.ok();
                                let email =
                                    antigravity::capture_current_account(&mut store_clone, live)?;
                                save_store(&store_clone)?;
                                format!("已捕获 Antigravity 账号: {email}")
                            }
                            Platform::AntigravityIde => {
                                let live = antigravity::fetch_ide_live_status().await.ok();
                                let email = antigravity::capture_current_ide_account(
                                    &mut store_clone,
                                    live,
                                )?;
                                save_store(&store_clone)?;
                                format!("已捕获 Antigravity IDE 账号: {email}")
                            }
                        };
                        Ok::<(AccountStore, String), String>((store_clone, msg))
                    })
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((updated_store, msg)) => {
                        this.store = updated_store;
                        this.status_message = msg;
                        this.update_tray_title();
                        this.refresh_all_quotas(cx);
                    }
                    Err(err) => {
                        this.status_message = err;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn export_accounts(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let default_dir = dirs::desktop_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let suggested = format!(
            "ai-switcher-backup-{}.json",
            chrono::Local::now().format("%Y%m%d")
        );
        let rx = cx.prompt_for_new_path(&default_dir, Some(&suggested));
        let store_snapshot = self.store.clone();

        cx.spawn(async move |this, cx| {
            let selected = rx.await.ok().and_then(|res| res.ok()).flatten();
            let Some(path) = selected else {
                return;
            };
            let save_res = cx
                .background_executor()
                .spawn(async move {
                    export_store_to_path(&store_snapshot, &path)?;
                    Ok::<std::path::PathBuf, String>(path)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                match save_res {
                    Ok(saved) => {
                        this.status_message = format!("已导出到 {}", saved.display());
                    }
                    Err(err) => {
                        this.status_message = format!("导出失败: {err}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn import_accounts(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("选择要导入的账号 JSON 文件".into()),
        });
        let mut store_clone = self.store.clone();

        cx.spawn(async move |this, cx| {
            let selected = rx.await.ok().and_then(|res| res.ok()).flatten();
            let Some(paths) = selected else {
                return;
            };
            if paths.is_empty() {
                return;
            }

            let import_res = cx
                .background_executor()
                .spawn(async move {
                    let mut total = ImportSummary::default();
                    for path in paths {
                        let s = import_accounts_from_path(&mut store_clone, &path)?;
                        total.codex_added += s.codex_added;
                        total.codex_updated += s.codex_updated;
                        total.antigravity_added += s.antigravity_added;
                        total.antigravity_updated += s.antigravity_updated;
                        total.ide_added += s.ide_added;
                        total.ide_updated += s.ide_updated;
                    }
                    save_store(&store_clone)?;
                    Ok::<(AccountStore, ImportSummary), String>((store_clone, total))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                match import_res {
                    Ok((updated_store, summary)) => {
                        this.store = updated_store;
                        this.status_message = summary.format_message();
                        this.update_tray_title();
                        this.refresh_all_quotas(cx);
                    }
                    Err(err) => {
                        this.status_message = format!("导入失败: {err}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn refresh_all_quotas(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status_message = "正在刷新额度...".to_string();
        cx.notify();

        let mut store_clone = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    tokio_rt().block_on(async move {
                        // 1. 刷新所有 Codex 账号额度
                        if let Ok(client) = codex::create_http_client() {
                            for account in &mut store_clone.codex_accounts {
                                match codex::fetch_usage(&client, &account.auth_json).await {
                                    Ok((usage, refreshed_auth)) => {
                                        if let Some(new_auth) = refreshed_auth {
                                            account.auth_json = new_auth;
                                        }
                                        if usage.plan_type.is_some() {
                                            account.plan_type = usage.plan_type.clone();
                                        }
                                        account.usage = Some(usage);
                                        account.usage_error = None;
                                    }
                                    Err(err) => {
                                        account.usage_error = Some(err);
                                    }
                                }
                            }
                        }

                        // 2. 探测本地运行中的 Antigravity 实时额度并自动同步
                        if let Ok(live) = antigravity::fetch_live_status().await {
                            let email = live.email.clone();
                            let _ = antigravity::capture_current_account(&mut store_clone, Some(live));
                            if let Some(found) = store_clone
                                .antigravity_accounts
                                .iter()
                                .find(|a| a.email.eq_ignore_ascii_case(&email))
                            {
                                store_clone.active_antigravity_id = Some(found.id.clone());
                            }
                        }

                        // 3. 探测本地运行中的 Antigravity IDE 实时额度并自动同步
                        if let Ok(live_ide) = antigravity::fetch_ide_live_status().await {
                            let email = live_ide.email.clone();
                            let _ = antigravity::capture_current_ide_account(
                                &mut store_clone,
                                Some(live_ide),
                            );
                            if let Some(found) = store_clone
                                .antigravity_ide_accounts
                                .iter()
                                .find(|a| a.email.eq_ignore_ascii_case(&email))
                            {
                                store_clone.active_antigravity_ide_id = Some(found.id.clone());
                            }
                        }

                        let _ = save_store(&store_clone);
                        store_clone
                    })
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.store = result;
                this.status_message = format!(
                    "已刷新 · {}",
                    chrono::Local::now().format("%H:%M:%S")
                );
                this.update_tray_title();
                cx.notify();
            });
        })
        .detach();
    }

    fn switch_codex(&mut self, id: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status_message = "正在切换 Codex 账号并重启桌面端...".to_string();
        cx.notify();

        let mut store_clone = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let name = codex::switch_account(&mut store_clone, &id)?;
                    Ok::<(AccountStore, String), String>((store_clone, name))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((updated, name)) => {
                        this.store = updated;
                        this.status_message = format!("已切换 Codex 至 {name}");
                        this.update_tray_title();
                    }
                    Err(err) => {
                        this.status_message = format!("切换失败: {err}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn switch_antigravity(&mut self, id: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status_message = "正在切换 Antigravity 账号并重启应用...".to_string();
        cx.notify();

        let mut store_clone = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let email = antigravity::switch_account(&mut store_clone, &id)?;
                    Ok::<(AccountStore, String), String>((store_clone, email))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((updated, email)) => {
                        this.store = updated;
                        this.status_message = format!("已切换 Antigravity 至 {email}");
                        this.update_tray_title();
                    }
                    Err(err) => {
                        this.status_message = format!("切换失败: {err}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn switch_antigravity_ide(&mut self, id: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status_message = "正在切换 Antigravity IDE 账号并重启 IDE...".to_string();
        cx.notify();

        let mut store_clone = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let email = antigravity::switch_ide_account(&mut store_clone, &id)?;
                    Ok::<(AccountStore, String), String>((store_clone, email))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((updated, email)) => {
                        this.store = updated;
                        this.status_message = format!("已切换 Antigravity IDE 至 {email}");
                        this.update_tray_title();
                    }
                    Err(err) => {
                        this.status_message = format!("切换失败: {err}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn delete_codex(&mut self, id: &str, cx: &mut Context<Self>) {
        self.store.codex_accounts.retain(|a| a.id != id);
        if self.store.active_codex_id.as_deref() == Some(id) {
            self.store.active_codex_id = None;
        }
        let _ = save_store(&self.store);
        self.status_message = "已移除 Codex 账号".to_string();
        self.update_tray_title();
        cx.notify();
    }

    fn delete_antigravity(&mut self, id: &str, cx: &mut Context<Self>) {
        self.store.antigravity_accounts.retain(|a| a.id != id);
        if self.store.active_antigravity_id.as_deref() == Some(id) {
            self.store.active_antigravity_id = None;
        }
        let _ = save_store(&self.store);
        self.status_message = "已移除 Antigravity 账号".to_string();
        self.update_tray_title();
        cx.notify();
    }

    fn delete_antigravity_ide(&mut self, id: &str, cx: &mut Context<Self>) {
        self.store.antigravity_ide_accounts.retain(|a| a.id != id);
        if self.store.active_antigravity_ide_id.as_deref() == Some(id) {
            self.store.active_antigravity_ide_id = None;
        }
        let _ = save_store(&self.store);
        self.status_message = "已移除 Antigravity IDE 账号".to_string();
        self.update_tray_title();
        cx.notify();
    }

    fn render_progress_bar(label: &str, remaining_pct: f64, reset_text: String) -> impl IntoElement {
        let clamped = remaining_pct.clamp(0.0, 100.0);
        let fill_color = quota_color(clamped);

        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(label.to_string()),
                            )
                            .child(
                                div()
                                    .text_color(fill_color)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("{clamped:.0}%")),
                            ),
                    )
                    .child(
                        div()
                            .text_color(rgb(TEXT_MUTED))
                            .child(format!("重置: {reset_text}")),
                    ),
            )
            .child(
                div()
                    .h(px(6.0))
                    .w_full()
                    .rounded(px(3.0))
                    .bg(rgb(0x16171a))
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(relative((clamped / 100.0) as f32))
                            .rounded(px(3.0))
                            .bg(fill_color),
                    ),
            )
    }

    fn render_codex_card(&self, account: &CodexAccount, cx: &mut Context<Self>) -> impl IntoElement {
        let is_active = self.store.active_codex_id.as_deref() == Some(account.id.as_str());
        let id_switch = account.id.clone();
        let id_delete = account.id.clone();

        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(14.0))
            .rounded(px(10.0))
            .bg(if is_active {
                rgb(BG_CARD_ACTIVE)
            } else {
                rgb(BG_CARD)
            })
            .border_1()
            .border_color(if is_active {
                rgb(BORDER_ACTIVE)
            } else {
                rgb(BORDER_SUBTLE)
            });

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .size(px(8.0))
                            .rounded(px(4.0))
                            .bg(if is_active {
                                rgb(ACCENT_GREEN)
                            } else {
                                rgb(TEXT_MUTED)
                            }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT_PRIMARY))
                            .child(account.display_name()),
                    )
                    .when_some(account.plan_type.clone(), |el, plan| {
                        el.child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(rgb(0x2e3440))
                                .text_xs()
                                .text_color(rgb(0x93c5fd))
                                .child(plan.to_uppercase()),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id(SharedString::from(format!("cx-switch-{}", account.id)))
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .bg(if is_active {
                                rgb(0x1e3a2f)
                            } else {
                                rgb(ACCENT_BLUE)
                            })
                            .text_color(if is_active {
                                rgb(0x86efac)
                            } else {
                                rgb(0xffffff)
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !is_active {
                                    this.switch_codex(id_switch.clone(), cx);
                                }
                            }))
                            .child(if is_active { "当前使用" } else { "切换" }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("cx-del-{}", account.id)))
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .text_xs()
                            .bg(rgb(0x2d2428))
                            .text_color(rgb(0xfca5a5))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_codex(&id_delete, cx);
                            }))
                            .child("移除"),
                    ),
            );

        card = card.child(header);

        if let Some(ref usage) = account.usage {
            let mut bars = div().flex().flex_col().gap(px(8.0));
            if let Some(ref w5) = usage.five_hour {
                bars = bars.child(Self::render_progress_bar(
                    "5h 额度",
                    w5.remaining_percent(),
                    format_reset_countdown(w5.reset_at),
                ));
            }
            if let Some(ref w1) = usage.one_week {
                bars = bars.child(Self::render_progress_bar(
                    "周额度",
                    w1.remaining_percent(),
                    format_reset_countdown(w1.reset_at),
                ));
            }
            card = card.child(bars);
        } else if let Some(ref err) = account.usage_error {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xfca5a5))
                    .child(err.clone()),
            );
        } else {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(rgb(TEXT_MUTED))
                    .child("等待刷新额度..."),
            );
        }

        card
    }

    fn render_antigravity_card(
        &self,
        account: &AntigravityAccount,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active = self.store.active_antigravity_id.as_deref() == Some(account.id.as_str());
        let id_switch = account.id.clone();
        let id_delete = account.id.clone();

        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(14.0))
            .rounded(px(10.0))
            .bg(if is_active {
                rgb(BG_CARD_ACTIVE)
            } else {
                rgb(BG_CARD)
            })
            .border_1()
            .border_color(if is_active {
                rgb(BORDER_ACTIVE)
            } else {
                rgb(BORDER_SUBTLE)
            });

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .size(px(8.0))
                            .rounded(px(4.0))
                            .bg(if is_active {
                                rgb(ACCENT_GREEN)
                            } else {
                                rgb(TEXT_MUTED)
                            }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT_PRIMARY))
                            .child(account.email.clone()),
                    )
                    .when_some(account.plan_name.clone(), |el, plan| {
                        el.child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(rgb(0x2e3440))
                                .text_xs()
                                .text_color(rgb(0xc4b5fd))
                                .child(plan),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id(SharedString::from(format!("ag-switch-{}", account.id)))
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .bg(if is_active {
                                rgb(0x1e3a2f)
                            } else {
                                rgb(ACCENT_BLUE)
                            })
                            .text_color(if is_active {
                                rgb(0x86efac)
                            } else {
                                rgb(0xffffff)
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !is_active {
                                    this.switch_antigravity(id_switch.clone(), cx);
                                }
                            }))
                            .child(if is_active { "当前使用" } else { "切换" }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("ag-del-{}", account.id)))
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .text_xs()
                            .bg(rgb(0x2d2428))
                            .text_color(rgb(0xfca5a5))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_antigravity(&id_delete, cx);
                            }))
                            .child("移除"),
                    ),
            );

        card = card.child(header);

        if let Some(ref usage) = account.usage {
            let mut bars = div().flex().flex_col().gap(px(8.0));
            for bucket in &usage.buckets {
                let reset = bucket
                    .reset_time
                    .as_deref()
                    .map(format_rfc3339_countdown)
                    .unwrap_or_else(|| "-".to_string());
                bars = bars.child(Self::render_progress_bar(
                    &bucket.label,
                    bucket.remaining_percent,
                    reset,
                ));
            }
            card = card.child(bars);
        } else if let Some(ref err) = account.usage_error {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xfca5a5))
                    .child(err.clone()),
            );
        } else {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(rgb(TEXT_MUTED))
                    .child("暂无额度快照（切换到该账号后自动采集）"),
            );
        }

        card
    }

    fn render_antigravity_ide_card(
        &self,
        account: &AntigravityIdeAccount,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active =
            self.store.active_antigravity_ide_id.as_deref() == Some(account.id.as_str());
        let id_switch = account.id.clone();
        let id_delete = account.id.clone();

        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(14.0))
            .rounded(px(10.0))
            .bg(if is_active {
                rgb(BG_CARD_ACTIVE)
            } else {
                rgb(BG_CARD)
            })
            .border_1()
            .border_color(if is_active {
                rgb(BORDER_ACTIVE)
            } else {
                rgb(BORDER_SUBTLE)
            });

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .size(px(8.0))
                            .rounded(px(4.0))
                            .bg(if is_active {
                                rgb(ACCENT_GREEN)
                            } else {
                                rgb(TEXT_MUTED)
                            }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT_PRIMARY))
                            .child(account.email.clone()),
                    )
                    .when_some(account.plan_name.clone(), |el, plan| {
                        el.child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(rgb(0x2e3440))
                                .text_xs()
                                .text_color(rgb(0x67e8f9))
                                .child(plan),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id(SharedString::from(format!("ide-switch-{}", account.id)))
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .bg(if is_active {
                                rgb(0x1e3a2f)
                            } else {
                                rgb(ACCENT_BLUE)
                            })
                            .text_color(if is_active {
                                rgb(0x86efac)
                            } else {
                                rgb(0xffffff)
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !is_active {
                                    this.switch_antigravity_ide(id_switch.clone(), cx);
                                }
                            }))
                            .child(if is_active { "当前使用" } else { "切换" }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("ide-del-{}", account.id)))
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .text_xs()
                            .bg(rgb(0x2d2428))
                            .text_color(rgb(0xfca5a5))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_antigravity_ide(&id_delete, cx);
                            }))
                            .child("移除"),
                    ),
            );

        card = card.child(header);

        if let Some(ref usage) = account.usage {
            let mut bars = div().flex().flex_col().gap(px(8.0));
            for bucket in &usage.buckets {
                let reset = bucket
                    .reset_time
                    .as_deref()
                    .map(format_rfc3339_countdown)
                    .unwrap_or_else(|| "-".to_string());
                bars = bars.child(Self::render_progress_bar(
                    &bucket.label,
                    bucket.remaining_percent,
                    reset,
                ));
            }
            card = card.child(bars);
        } else if let Some(ref err) = account.usage_error {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xfca5a5))
                    .child(err.clone()),
            );
        } else {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(rgb(TEXT_MUTED))
                    .child("暂无额度快照（打开 Antigravity IDE 后自动采集）"),
            );
        }

        card
    }
}

impl Render for MainView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_codex = self.platform == Platform::Codex;
        let is_ag = self.platform == Platform::Antigravity;
        let is_ide = self.platform == Platform::AntigravityIde;

        let titlebar = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(48.0))
            .pl(px(if cfg!(target_os = "macos") { 82.0 } else { 16.0 }))
            .pr(px(16.0))
            .bg(rgb(BG_SURFACE))
            .border_b_1()
            .border_color(rgb(BORDER_SUBTLE))
            .child(
                // 平台切换 Segmented Control
                div()
                    .flex()
                    .flex_row()
                    .p(px(3.0))
                    .rounded(px(8.0))
                    .bg(rgb(BG_WINDOW))
                    .gap(px(2.0))
                    .child(
                        div()
                            .id("tab-codex")
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .bg(if is_codex {
                                rgb(0x323642)
                            } else {
                                rgb(BG_WINDOW)
                            })
                            .text_color(if is_codex {
                                rgb(TEXT_PRIMARY)
                            } else {
                                rgb(TEXT_SECONDARY)
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.platform = Platform::Codex;
                                cx.notify();
                            }))
                            .child(format!("Codex ({})", self.store.codex_accounts.len())),
                    )
                    .child(
                        div()
                            .id("tab-antigravity")
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .bg(if is_ag {
                                rgb(0x323642)
                            } else {
                                rgb(BG_WINDOW)
                            })
                            .text_color(if is_ag {
                                rgb(TEXT_PRIMARY)
                            } else {
                                rgb(TEXT_SECONDARY)
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.platform = Platform::Antigravity;
                                cx.notify();
                            }))
                            .child(format!(
                                "Antigravity ({})",
                                self.store.antigravity_accounts.len()
                            )),
                    )
                    .child(
                        div()
                            .id("tab-antigravity-ide")
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .bg(if is_ide {
                                rgb(0x323642)
                            } else {
                                rgb(BG_WINDOW)
                            })
                            .text_color(if is_ide {
                                rgb(TEXT_PRIMARY)
                            } else {
                                rgb(TEXT_SECONDARY)
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.platform = Platform::AntigravityIde;
                                cx.notify();
                            }))
                            .child(format!(
                                "IDE ({})",
                                self.store.antigravity_ide_accounts.len()
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id("btn-oauth-login")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(rgb(0x1f3a2e))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(0x86efac))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.authorize_new_account(cx);
                            }))
                            .child("登录授权"),
                    )
                    .child(
                        div()
                            .id("btn-capture")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(rgb(0x2b2f3a))
                            .text_xs()
                            .text_color(rgb(TEXT_PRIMARY))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.capture_current(cx);
                            }))
                            .child("捕获当前"),
                    )
                    .child(
                        div()
                            .id("btn-import")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(rgb(0x2b2f3a))
                            .text_xs()
                            .text_color(rgb(TEXT_PRIMARY))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.import_accounts(cx);
                            }))
                            .child("导入"),
                    )
                    .child(
                        div()
                            .id("btn-export")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(rgb(0x2b2f3a))
                            .text_xs()
                            .text_color(rgb(TEXT_PRIMARY))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export_accounts(cx);
                            }))
                            .child("导出"),
                    )
                    .child(
                        div()
                            .id("btn-refresh")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(rgb(ACCENT_BLUE))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(0xffffff))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh_all_quotas(cx);
                            }))
                            .child(if self.busy { "处理中..." } else { "刷新" }),
                    ),
            );

        let mut list = div()
            .id("account-list")
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(16.0))
            .overflow_y_scroll();

        match self.platform {
            Platform::Codex => {
                if self.store.codex_accounts.is_empty() {
                    list = list.child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .py(px(60.0))
                            .gap(px(8.0))
                            .text_sm()
                            .text_color(rgb(TEXT_MUTED))
                            .child("暂无 Codex 账号，点击右上角「登录授权」「捕获当前」或「导入」"),
                    );
                } else {
                    for acc in &self.store.codex_accounts {
                        list = list.child(self.render_codex_card(acc, cx));
                    }
                }
            }
            Platform::Antigravity => {
                if self.store.antigravity_accounts.is_empty() {
                    list = list.child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .py(px(60.0))
                            .gap(px(8.0))
                            .text_sm()
                            .text_color(rgb(TEXT_MUTED))
                            .child("暂无 Antigravity 账号，打开 Antigravity 登录后点击「捕获当前」或「登录授权」"),
                    );
                } else {
                    for acc in &self.store.antigravity_accounts {
                        list = list.child(self.render_antigravity_card(acc, cx));
                    }
                }
            }
            Platform::AntigravityIde => {
                if self.store.antigravity_ide_accounts.is_empty() {
                    list = list.child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .py(px(60.0))
                            .gap(px(8.0))
                            .text_sm()
                            .text_color(rgb(TEXT_MUTED))
                            .child("暂无 Antigravity IDE 账号，点击「捕获当前」或「登录授权」录入"),
                    );
                } else {
                    for acc in &self.store.antigravity_ide_accounts {
                        list = list.child(self.render_antigravity_ide_card(acc, cx));
                    }
                }
            }
        }

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(30.0))
            .px(px(16.0))
            .bg(rgb(BG_SURFACE))
            .border_t_1()
            .border_color(rgb(BORDER_SUBTLE))
            .text_xs()
            .text_color(rgb(TEXT_SECONDARY))
            .child(self.status_message.clone())
            .child(
                div()
                    .text_color(rgb(TEXT_MUTED))
                    .child("GPUI · 60s 自动轮询"),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG_WINDOW))
            .text_color(rgb(TEXT_PRIMARY))
            .child(titlebar)
            .child(list)
            .child(footer)
    }
}
