#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod antigravity;
mod codex;
mod models;
mod process;
mod store;
mod tray;
mod ui;
mod update;

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

#[cfg(target_os = "windows")]
mod win32_window {
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    const SW_HIDE: i32 = 0;
    const SW_SHOW: i32 = 5;
    const SW_RESTORE: i32 = 9;
    const WM_SETICON: u32 = 0x0080;
    const ICON_SMALL: usize = 0;
    const ICON_BIG: usize = 1;
    const IMAGE_ICON: u32 = 1;
    const LR_DEFAULTSIZE: u32 = 0x0000_0040;
    const LR_SHARED: u32 = 0x0000_8000;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(module_name: *const u16) -> isize;
    }

    #[link(name = "user32")]
    extern "system" {
        fn ShowWindow(hwnd: isize, n_cmd_show: i32) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
        fn LoadImageW(
            hinst: isize,
            name: usize,
            r#type: u32,
            cx: i32,
            cy: i32,
            fu_load: u32,
        ) -> isize;
        fn SendMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    }

    fn hwnd_of(window: &Window) -> Option<isize> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        }
    }

    pub fn apply_window_icon(window: &Window) {
        let Some(hwnd) = hwnd_of(window) else {
            return;
        };
        unsafe {
            let hinst = GetModuleHandleW(std::ptr::null());
            let big = LoadImageW(hinst, 1, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE | LR_SHARED);
            let small = LoadImageW(hinst, 1, IMAGE_ICON, 16, 16, LR_SHARED);
            if big != 0 {
                SendMessageW(hwnd, WM_SETICON, ICON_BIG, big);
            }
            if small != 0 {
                SendMessageW(hwnd, WM_SETICON, ICON_SMALL, small);
            }
        }
    }

    pub fn hide_to_tray(window: &Window) {
        if let Some(hwnd) = hwnd_of(window) {
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    pub fn show_from_tray(window: &Window) {
        if let Some(hwnd) = hwnd_of(window) {
            unsafe {
                ShowWindow(hwnd, SW_SHOW);
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
        }
    }
}

fn open_main_window(cx: &mut App) {
    let tray = cx
        .try_global::<AppGlobal>()
        .and_then(|g| g.tray.clone());

    let bounds = Bounds::centered(None, size(px(720.0), px(520.0)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("AI Switcher".into()),
            appears_transparent: cfg!(target_os = "macos"),
            traffic_light_position: cfg!(target_os = "macos")
                .then_some(point(px(14.0), px(16.0))),
        }),
        window_min_size: Some(size(px(600.0), px(400.0))),
        ..Default::default()
    };

    if let Ok(handle) = cx.open_window(options, |window, cx| {
        #[cfg(target_os = "windows")]
        {
            win32_window::apply_window_icon(window);
            window.on_window_should_close(cx, |window, _cx| {
                win32_window::hide_to_tray(window);
                false
            });
        }
        let _ = window;
        cx.new(|cx| MainView::new(tray, cx))
    }) {
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
            #[cfg(target_os = "windows")]
            win32_window::show_from_tray(window);
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
