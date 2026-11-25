#[cfg(any(target_os = "macos", target_os = "windows"))]
use muda::{Menu, MenuEvent, MenuItem};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::models::AccountStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ShowWindow,
    RefreshQuotas,
    Quit,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub struct TrayController {
    tray: TrayIcon,
    show_id: muda::MenuId,
    refresh_id: muda::MenuId,
    quit_id: muda::MenuId,
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub struct TrayController;

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl TrayController {
    pub fn new() -> Result<Self, String> {
        let menu = Menu::new();
        let show_item = MenuItem::new("打开主窗口", true, None);
        let refresh_item = MenuItem::new("立即刷新额度", true, None);
        let quit_item = MenuItem::new("退出 AI Switcher", true, None);

        let show_id = show_item.id().clone();
        let refresh_id = refresh_item.id().clone();
        let quit_id = quit_item.id().clone();

        menu.append(&show_item).map_err(|e| e.to_string())?;
        menu.append(&refresh_item).map_err(|e| e.to_string())?;
        menu.append(&quit_item).map_err(|e| e.to_string())?;

        #[cfg(target_os = "macos")]
        let icon = build_template_icon()?;
        #[cfg(target_os = "windows")]
        let icon = build_windows_tray_icon()?;

        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_icon(icon)
            .with_tooltip("AI Switcher — 左键打开窗口 / 右键菜单");

        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_as_template(true).with_title(" AI");

        let tray = builder
            .build()
            .map_err(|e| format!("创建菜单栏托盘失败: {e}"))?;

        Ok(Self {
            tray,
            show_id,
            refresh_id,
            quit_id,
        })
    }

    pub fn set_title(&self, title: &str) {
        #[cfg(target_os = "macos")]
        self.tray.set_title(Some(title));
        let _ = self
            .tray
            .set_tooltip(Some(format!("AI Switcher — {}", title.trim())));
    }

    pub fn update_from_store(&self, store: &AccountStore) {
        self.set_title(&format_tray_title(store));
    }

    pub fn poll_actions(&self) -> Vec<TrayAction> {
        let mut actions = Vec::new();

        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                actions.push(TrayAction::ShowWindow);
            }
        }

        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.show_id {
                actions.push(TrayAction::ShowWindow);
            } else if ev.id == self.refresh_id {
                actions.push(TrayAction::RefreshQuotas);
            } else if ev.id == self.quit_id {
                actions.push(TrayAction::Quit);
            }
        }

        actions
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl TrayController {
    pub fn new() -> Result<Self, String> {
        Err("Linux 暂不启用系统托盘".to_string())
    }

    pub fn update_from_store(&self, _store: &AccountStore) {}

    pub fn poll_actions(&self) -> Vec<TrayAction> {
        Vec::new()
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn format_tray_title(store: &AccountStore) -> String {
    let cx_part = store
        .active_codex_id
        .as_deref()
        .and_then(|id| store.codex_accounts.iter().find(|a| a.id == id))
        .or_else(|| store.codex_accounts.first())
        .and_then(|a| a.usage.as_ref())
        .and_then(|u| {
            let vals: Vec<f64> = [u.five_hour.as_ref(), u.one_week.as_ref()]
                .into_iter()
                .flatten()
                .map(|w| w.remaining_percent())
                .collect();
            vals.into_iter().reduce(f64::min)
        })
        .map(|pct| format!("CX {:.0}%", pct));

    let ag_part = store
        .active_antigravity_id
        .as_deref()
        .and_then(|id| store.antigravity_accounts.iter().find(|a| a.id == id))
        .or_else(|| store.antigravity_accounts.first())
        .and_then(|a| a.usage.as_ref())
        .and_then(|u| {
            u.buckets
                .iter()
                .map(|b| b.remaining_percent)
                .reduce(f64::min)
        })
        .map(|pct| format!("AG {:.0}%", pct));

    match (cx_part, ag_part) {
        (Some(c), Some(a)) => format!(" {c} · {a}"),
        (Some(c), None) => format!(" {c}"),
        (None, Some(a)) => format!(" {a}"),
        (None, None) => " AI".to_string(),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn dist_to_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let abx = bx - ax;
    let aby = by - ay;
    let apx = px - ax;
    let apy = py - ay;
    let len_sq = abx * abx + aby * aby;
    let t = if len_sq > 0.0 {
        ((apx * abx + apy * aby) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let cx = ax + t * abx;
    let cy = ay + t * aby;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// 36x36 macOS Retina 极简模板图标：双向切换箭头（⇄），与 App 图标一致
#[cfg(target_os = "macos")]
fn build_template_icon() -> Result<Icon, String> {
    let size = 36u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let half_w = 1.65f32;

    let segments: [(f32, f32, f32, f32); 6] = [
        (7.5, 12.0, 28.5, 12.0),
        (22.5, 6.5, 28.5, 12.0),
        (22.5, 17.5, 28.5, 12.0),
        (28.5, 24.0, 7.5, 24.0),
        (13.5, 18.5, 7.5, 24.0),
        (13.5, 29.5, 7.5, 24.0),
    ];

    for y in 0..size {
        for x in 0..size {
            let mut coverage = 0.0f32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                    let hit = segments
                        .iter()
                        .any(|&(ax, ay, bx, by)| dist_to_segment(px, py, ax, ay, bx, by) <= half_w);
                    if hit {
                        coverage += 1.0;
                    }
                }
            }
            let alpha = ((coverage / 16.0) * 255.0).round() as u8;
            let idx = ((y * size + x) * 4) as usize;
            rgba[idx] = 0;
            rgba[idx + 1] = 0;
            rgba[idx + 2] = 0;
            rgba[idx + 3] = alpha;
        }
    }

    Icon::from_rgba(rgba, size, size).map_err(|e| format!("构建托盘图标失败: {e}"))
}

/// 32x32 Windows 标准彩色托盘图标：深石墨圆角底座 + 白/蓝双向箭头（深浅任务栏均清晰可见）
#[cfg(target_os = "windows")]
fn build_windows_tray_icon() -> Result<Icon, String> {
    let size = 32u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let half_w = 1.55f32;

    let top_segments: [(f32, f32, f32, f32); 3] = [
        (7.5, 11.2, 24.5, 11.2),
        (19.5, 6.8, 24.5, 11.2),
        (19.5, 15.6, 24.5, 11.2),
    ];
    let bot_segments: [(f32, f32, f32, f32); 3] = [
        (24.5, 20.8, 7.5, 20.8),
        (12.5, 16.4, 7.5, 20.8),
        (12.5, 25.2, 7.5, 20.8),
    ];

    for y in 0..size {
        for x in 0..size {
            let mut r_acc = 0.0f32;
            let mut g_acc = 0.0f32;
            let mut b_acc = 0.0f32;
            let mut a_acc = 0.0f32;

            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let py = y as f32 + (sy as f32 + 0.5) / 4.0;

                    // 圆角矩形底座 [1.5..30.5], radius = 6.5
                    let cx = (px - 16.0).abs() - 8.0;
                    let cy = (py - 16.0).abs() - 8.0;
                    let rounded_dist = (cx.max(0.0).powi(2) + cy.max(0.0).powi(2)).sqrt()
                        + cx.max(cy).min(0.0)
                        - 6.5;

                    if rounded_dist <= 0.0 {
                        let hit_top = top_segments
                            .iter()
                            .any(|&(ax, ay, bx, by)| dist_to_segment(px, py, ax, ay, bx, by) <= half_w);
                        let hit_bot = bot_segments
                            .iter()
                            .any(|&(ax, ay, bx, by)| dist_to_segment(px, py, ax, ay, bx, by) <= half_w);

                        let (r, g, b) = if hit_top {
                            (245.0, 248.0, 252.0)
                        } else if hit_bot {
                            (64.0, 158.0, 255.0)
                        } else {
                            (30.0, 34.0, 44.0)
                        };
                        r_acc += r;
                        g_acc += g;
                        b_acc += b;
                        a_acc += 1.0;
                    }
                }
            }

            let idx = ((y * size + x) * 4) as usize;
            if a_acc > 0.0 {
                rgba[idx] = (r_acc / a_acc).round() as u8;
                rgba[idx + 1] = (g_acc / a_acc).round() as u8;
                rgba[idx + 2] = (b_acc / a_acc).round() as u8;
                rgba[idx + 3] = ((a_acc / 16.0) * 255.0).round() as u8;
            }
        }
    }

    Icon::from_rgba(rgba, size, size).map_err(|e| format!("构建 Windows 托盘图标失败: {e}"))
}
