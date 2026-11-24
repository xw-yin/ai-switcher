mod antigravity;
mod codex;
mod models;
mod process;
mod store;
mod tray;
mod ui;

use std::rc::Rc;
use std::time::Duration;

use gpui::*;

use crate::tray::{TrayAction, TrayController};
use crate::ui::MainView;

pub struct AppGlobal {
    pub tray: Option<Rc<TrayController>>,
    pub main_view: Option<WeakEntity<MainView>>,
}

impl Global for AppGlobal {}

fn open_main_window(cx: &mut App) {
    let tray = cx
        .try_global::<AppGlobal>()
        .and_then(|g| g.tray.clone());

    let bounds = Bounds::centered(None, size(px(720.0), px(520.0)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("AI Switcher".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(14.0), px(16.0))),
        }),
        window_min_size: Some(size(px(600.0), px(400.0))),
        ..Default::default()
    };

    if let Ok(handle) = cx.open_window(options, |_window, cx| cx.new(|cx| MainView::new(tray, cx))) {
        let weak = handle
            .update(cx, |_view, window, cx| {
                window.activate_window();
                cx.entity().downgrade()
            })
            .ok();
        if cx.has_global::<AppGlobal>() {
            cx.global_mut::<AppGlobal>().main_view = weak;
        }
    }
    cx.activate(true);
}

fn show_or_open_main_window(cx: &mut App) {
    cx.activate(true);
    if let Some(window_handle) = cx.windows().into_iter().next() {
        let _ = window_handle.update(cx, |_, window, _| {
            window.activate_window();
        });
    } else {
        open_main_window(cx);
    }
}

fn trigger_refresh(cx: &mut App) {
    let maybe_view = cx
        .try_global::<AppGlobal>()
        .and_then(|g| g.main_view.as_ref().and_then(|w| w.upgrade()));

    if let Some(view) = maybe_view {
        view.update(cx, |this, cx| {
            this.refresh_all_quotas(cx);
        });
        return;
    }

    // 窗口关闭处于纯后台模式时，直接无头刷新额度并更新菜单栏标题
    let tray = cx.try_global::<AppGlobal>().and_then(|g| g.tray.clone());
    cx.spawn(async move |cx| {
        let updated = cx
            .background_executor()
            .spawn(async move { ui::refresh_store_quotas_blocking() })
            .await;
        if let Some(store) = updated {
            if let Some(tray) = tray {
                tray.update_from_store(&store);
            }
        }
    })
    .detach();
}

fn main() {
    let app = Application::new();
    app.on_reopen(|cx| {
        show_or_open_main_window(cx);
    });
    app.run(|cx: &mut App| {
            let tray = TrayController::new().ok().map(Rc::new);
            cx.set_global(AppGlobal {
                tray: tray.clone(),
                main_view: None,
            });

            open_main_window(cx);

            // App 级托盘事件循环：即使主窗口关闭也持续响应左键/右键菜单
            cx.spawn(async move |cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
                let res = cx.update(|cx| {
                    let Some(tray) = cx
                        .try_global::<AppGlobal>()
                        .and_then(|g| g.tray.clone())
                    else {
                        return;
                    };
                    for action in tray.poll_actions() {
                        match action {
                            TrayAction::ShowWindow => show_or_open_main_window(cx),
                            TrayAction::RefreshQuotas => trigger_refresh(cx),
                            TrayAction::Quit => cx.quit(),
                        }
                    }
                });
                if res.is_err() {
                    break;
                }
            })
            .detach();

            // App 级 60s 定时额度轮询（窗口关闭时仍保持菜单栏实时刷新）
            cx.spawn(async move |cx| loop {
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
                let res = cx.update(|cx| {
                    trigger_refresh(cx);
                });
                if res.is_err() {
                    break;
                }
            })
            .detach();
        });
}
