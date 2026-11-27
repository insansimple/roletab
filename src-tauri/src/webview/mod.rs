pub mod bridge;
pub mod platform;

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::webview::{DownloadEvent, NewWindowFeatures, NewWindowResponse, WebviewBuilder};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Rect, Runtime, WebviewUrl,
    WebviewWindowBuilder, WindowBuilder, WindowEvent,
};
use uuid::Uuid;

use crate::downloads::{resolve_download_target_dir, resolve_unique_download_path, reveal_in_explorer};
use crate::profile::{
    build_profile_proxy_url, create_profile_record, delete_dir_with_retry, find_profile_record,
    profile_data_dir, sync_profiles_store, touch_profile_last_used, EPHEMERAL_BADGE_COLOR,
};
use crate::profile_transfer::export_profile_dir_to_zip;
use crate::state::{AppState, DownloadEntry, DownloadStatus, TabCrashKind, TabEntry, UiDensity};
use crate::workspace::load_workspaces;
use bridge::NativeNavState;
use platform::PlatformWebviewController;

/// Compact 2-row Chrome-like UI Shell height in Comfortable mode (36px Tab Strip + 42px Toolbar).
pub const UI_SHELL_HEIGHT: f64 = 78.0;

/// UI Shell height when Find-in-Page floating bar (`Ctrl+F`) is docked beneath Comfortable toolbar.
pub const UI_SHELL_FIND_HEIGHT: f64 = 116.0;

/// Compact density UI Shell height (30px Tab Strip + 36px Toolbar).
pub const UI_SHELL_COMPACT_HEIGHT: f64 = 66.0;

/// Compact density UI Shell height with docked Find-in-Page bar.
pub const UI_SHELL_COMPACT_FIND_HEIGHT: f64 = 104.0;

/// Returns effective logical UI Shell height based on Find-in-Page visibility and UI density.
pub fn shell_height_for_prefs(find_open: bool, density: UiDensity) -> f64 {
    match (density, find_open) {
        (UiDensity::Compact, true) => UI_SHELL_COMPACT_FIND_HEIGHT,
        (UiDensity::Compact, false) => UI_SHELL_COMPACT_HEIGHT,
        (UiDensity::Comfortable, true) => UI_SHELL_FIND_HEIGHT,
        (UiDensity::Comfortable, false) => UI_SHELL_HEIGHT,
    }
}

/// Returns effective logical UI Shell height in default Comfortable density.
#[cfg(test)]
pub fn shell_height_for_find(find_open: bool) -> f64 {
    shell_height_for_prefs(find_open, UiDensity::Comfortable)
}

/// Logical width of the Profile Manager side panel when open.
pub const PROFILE_PANEL_WIDTH: f64 = 336.0;

/// Logical width of the Download Manager floating drawer when open.
pub const DOWNLOAD_PANEL_WIDTH: f64 = 340.0;

/// Logical width of the Task Monitor & Sleep Settings floating drawer when open.
pub const MONITOR_PANEL_WIDTH: f64 = 360.0;

/// Default startup / Home URL for tabs.
pub const DEFAULT_NEW_TAB_URL: &str = "about:blank";

static POPUP_COUNTER: AtomicU64 = AtomicU64::new(1);
static WINDOW_COUNTER: AtomicU64 = AtomicU64::new(2);
static PANEL_ANIM_GEN: AtomicU64 = AtomicU64::new(1);
static DOWNLOAD_ANIM_GEN: AtomicU64 = AtomicU64::new(1);
static MONITOR_ANIM_GEN: AtomicU64 = AtomicU64::new(1);
static DOWNLOAD_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Returns the current Unix timestamp in whole seconds.
pub fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Computes the target physical width for the side panel when fully open.
pub fn compute_target_side_width(window_width: u32, scale_factor: f64) -> u32 {
    let scale = if scale_factor > 0.0 { scale_factor } else { 1.0 };
    let width = window_width.max(320);
    let min_viewport = (260.0 * scale).round() as u32;
    let max_side = width.saturating_sub(min_viewport).max(160);
    ((PROFILE_PANEL_WIDTH * scale).round() as u32)
        .min(max_side)
        .max(180)
}

/// Parameterized slide rect helper honoring dynamic `shell_logical_h` (`78` normal or `116` with Find bar).
pub fn compute_slide_step_rects_with_shell(
    window_width: u32,
    window_height: u32,
    scale_factor: f64,
    visible_side_width: u32,
    shell_logical_h: f64,
) -> (
    Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>,
    (PhysicalPosition<i32>, PhysicalSize<u32>),
) {
    let scale = if scale_factor > 0.0 { scale_factor } else { 1.0 };
    let width = window_width.max(320);
    let shell_height =
        ((shell_logical_h * scale).round() as u32).min(window_height.saturating_sub(80).max(1));
    let content_height = window_height.saturating_sub(shell_height).max(80);
    let target_side_w = compute_target_side_width(width, scale);
    let clamped_vis = visible_side_width.min(target_side_w);

    let viewport = (
        PhysicalPosition::new(0_i32, shell_height as i32),
        PhysicalSize::new(width, content_height),
    );

    if clamped_vis == 0 {
        (None, viewport)
    } else {
        let side_x = (clamped_vis as i32) - (target_side_w as i32);
        (
            Some((
                PhysicalPosition::new(side_x, shell_height as i32),
                PhysicalSize::new(target_side_w, content_height),
            )),
            viewport,
        )
    }
}

/// Computes exact `(side_pos, side_size, viewport_pos, viewport_size)` in physical pixels for any
/// intermediate `visible_side_width` during slide-open / slide-close animations.
#[cfg(test)]
pub fn compute_slide_step_rects(
    window_width: u32,
    window_height: u32,
    scale_factor: f64,
    visible_side_width: u32,
) -> (
    Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>,
    (PhysicalPosition<i32>, PhysicalSize<u32>),
) {
    compute_slide_step_rects_with_shell(
        window_width,
        window_height,
        scale_factor,
        visible_side_width,
        UI_SHELL_HEIGHT,
    )
}

/// Parameterized physical layout rects helper honoring dynamic `shell_logical_h`.
pub fn compute_physical_side_layout_rects_with_shell(
    window_width: u32,
    window_height: u32,
    scale_factor: f64,
    side_open: bool,
    shell_logical_h: f64,
) -> (Rect, Option<Rect>, Rect) {
    let scale = if scale_factor > 0.0 { scale_factor } else { 1.0 };
    let width = window_width.max(320);
    let shell_height =
        ((shell_logical_h * scale).round() as u32).min(window_height.saturating_sub(80).max(1));
    let target_side_w = if side_open {
        compute_target_side_width(width, scale)
    } else {
        0
    };
    let (side_opt, (vp_pos, vp_sz)) = compute_slide_step_rects_with_shell(
        width,
        window_height,
        scale,
        target_side_w,
        shell_logical_h,
    );

    let shell_rect = Rect {
        position: PhysicalPosition::new(0_i32, 0_i32).into(),
        size: PhysicalSize::new(width, shell_height).into(),
    };
    let side_rect = side_opt.map(|(p, s)| Rect {
        position: p.into(),
        size: s.into(),
    });
    let viewport_rect = Rect {
        position: vp_pos.into(),
        size: vp_sz.into(),
    };
    (shell_rect, side_rect, viewport_rect)
}

/// Computes exact physical pixel `Rect`s for `(ui_shell, optional_side_panel, content_viewport)`
/// from the parent window's physical `inner_size` and `scale_factor`.
#[cfg(test)]
pub fn compute_physical_side_layout_rects(
    window_width: u32,
    window_height: u32,
    scale_factor: f64,
    side_open: bool,
) -> (Rect, Option<Rect>, Rect) {
    compute_physical_side_layout_rects_with_shell(
        window_width,
        window_height,
        scale_factor,
        side_open,
        UI_SHELL_HEIGHT,
    )
}

/// Computes the target physical width for a right floating drawer with logical width `logical_width`.
pub fn compute_right_panel_target_width(
    window_width: u32,
    scale_factor: f64,
    logical_width: f64,
) -> u32 {
    let scale = if scale_factor > 0.0 { scale_factor } else { 1.0 };
    let width = window_width.max(320);
    let min_viewport = (260.0 * scale).round() as u32;
    let max_side = width.saturating_sub(min_viewport).max(160);
    ((logical_width * scale).round() as u32)
        .min(max_side)
        .max(180)
}

/// Computes `(pos, size)` in physical pixels for a right floating drawer at `visible_width` & `shell_logical_h`.
pub fn compute_right_panel_step_rect_with_shell(
    window_width: u32,
    window_height: u32,
    scale_factor: f64,
    logical_width: f64,
    visible_width: u32,
    shell_logical_h: f64,
) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    let scale = if scale_factor > 0.0 { scale_factor } else { 1.0 };
    let width = window_width.max(320);
    let shell_height =
        ((shell_logical_h * scale).round() as u32).min(window_height.saturating_sub(80).max(1));
    let content_height = window_height.saturating_sub(shell_height).max(1);
    let target_w = compute_right_panel_target_width(width, scale, logical_width);
    let clamped_vis = visible_width.min(target_w);

    if clamped_vis == 0 {
        None
    } else {
        let panel_x = (width as i32) - (clamped_vis as i32);
        Some((
            PhysicalPosition::new(panel_x, shell_height as i32),
            PhysicalSize::new(target_w, content_height),
        ))
    }
}

/// Computes `(pos, size)` in physical pixels for a right floating drawer at `visible_width`.
#[cfg(test)]
pub fn compute_right_panel_step_rect(
    window_width: u32,
    window_height: u32,
    scale_factor: f64,
    logical_width: f64,
    visible_width: u32,
) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    compute_right_panel_step_rect_with_shell(
        window_width,
        window_height,
        scale_factor,
        logical_width,
        visible_width,
        UI_SHELL_HEIGHT,
    )
}

/// Computes exact physical pixel `Rect`s for `(ui_shell, content_viewport)` from the parent window's
/// physical `inner_size` and `scale_factor`, avoiding logical/physical DPI round-trip mismatches in WebView2.
#[cfg(test)]
pub fn compute_physical_layout_rects(
    window_width: u32,
    window_height: u32,
    scale_factor: f64,
) -> (Rect, Rect) {
    let (shell_rect, _, viewport_rect) =
        compute_physical_side_layout_rects(window_width, window_height, scale_factor, false);
    (shell_rect, viewport_rect)
}

/// Checks if an address host (without scheme) targets `localhost` or a local/private IPv4 address.
pub fn is_local_or_private_host(raw: &str) -> bool {
    let host_port = raw.split('/').next().unwrap_or(raw);
    let host = host_port
        .strip_prefix('[')
        .and_then(|s| s.split(']').next())
        .unwrap_or_else(|| host_port.split(':').next().unwrap_or(host_port));

    if host.eq_ignore_ascii_case("localhost") || host == "::1" || host.ends_with(".local") {
        return true;
    }

    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return ip.is_loopback() || ip.is_private() || ip.is_link_local();
    }

    false
}

/// Normalizes user-typed address bar input into a navigable `url::Url`.
pub fn normalize_address_input(raw: &str) -> Result<url::Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("URL kosong".to_string());
    }
    let normalized = if trimmed == "about:blank"
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
    {
        trimmed.to_string()
    } else if is_local_or_private_host(trimmed) {
        format!("http://{trimmed}")
    } else {
        format!("https://{trimmed}")
    };

    normalized
        .parse::<url::Url>()
        .map_err(|e| format!("URL tidak valid: {e}"))
}

/// Returns the `ui-shell` child webview label belonging to `win_label`.
pub fn shell_label_for_window(win_label: &str) -> String {
    if win_label == "main" || win_label.is_empty() {
        "ui-shell".to_string()
    } else {
        format!("ui-shell--{win_label}")
    }
}

/// Returns the child panel webview label (`profile-panel`, `download-panel`, `task-monitor`) for `win_label`.
pub fn panel_label_for_window(base: &str, win_label: &str) -> String {
    if win_label == "main" || win_label.is_empty() {
        base.to_string()
    } else {
        format!("{base}--{win_label}")
    }
}

/// Resolves owning window label (`main`, `main-2`, ...) from a child shell/panel webview label.
pub fn window_label_from_webview_label(wv_label: &str) -> String {
    wv_label
        .split_once("--")
        .map(|(_, win)| win.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".to_string())
}

/// Resolves the `ui-shell` child label currently hosting `tab_id`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn shell_label_for_tab<R: Runtime>(app: &AppHandle<R>, tab_id: &str) -> String {
    let win_label = app
        .state::<AppState>()
        .tab_window_label(tab_id)
        .unwrap_or_else(|| "main".to_string());
    shell_label_for_window(&win_label)
}

/// Reorders tabs belonging strictly to `win_label` without disturbing tabs in other windows.
pub fn reorder_window_tabs_vec(
    tabs: &mut [TabEntry],
    win_label: &str,
    from_idx: usize,
    to_idx: usize,
) -> bool {
    let positions: Vec<usize> = tabs
        .iter()
        .enumerate()
        .filter(|(_, t)| t.window_label == win_label)
        .map(|(i, _)| i)
        .collect();
    if from_idx >= positions.len() || to_idx >= positions.len() || from_idx == to_idx {
        return false;
    }
    let mut win_tabs: Vec<TabEntry> = positions.iter().map(|&i| tabs[i].clone()).collect();
    let item = win_tabs.remove(from_idx);
    win_tabs.insert(to_idx, item);
    for (slot_idx, orig_pos) in positions.into_iter().enumerate() {
        tabs[orig_pos] = win_tabs[slot_idx].clone();
    }
    true
}

/// Reorders a slice of tabs safely when dragging a tab from `from_idx` to `to_idx`.
#[cfg(test)]
pub fn reorder_tabs_vec<T>(tabs: &mut Vec<T>, from_idx: usize, to_idx: usize) -> bool {
    if from_idx >= tabs.len() || to_idx >= tabs.len() || from_idx == to_idx {
        return false;
    }
    let item = tabs.remove(from_idx);
    tabs.insert(to_idx, item);
    true
}

/// Determines whether `on_new_window` represents a script popup (e.g., OAuth `window.open`)
/// rather than a standard `target="_blank"` tab navigation.
pub fn is_popup_features(features: &NewWindowFeatures) -> bool {
    features.size().is_some() || features.position().is_some()
}

pub fn emit_shell_state<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    let win_labels = state.window_labels();
    for win_label in win_labels {
        let snapshot = state.snapshot_for_window(&win_label);
        let shell_lbl = shell_label_for_window(&win_label);
        let _ = app.emit_to(&shell_lbl, "shell-state", &snapshot);
        for base in ["profile-panel", "download-panel", "task-monitor"] {
            let panel_lbl = panel_label_for_window(base, &win_label);
            if app.get_webview(&panel_lbl).is_some() {
                let _ = app.emit_to(&panel_lbl, "shell-state", &snapshot);
            }
        }
    }
}

/// Applies native WebView2 navigation/favicon/certificate/audio/PID updates into `AppState`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn apply_native_nav_update<R: Runtime>(app: &AppHandle<R>, update: NativeNavState) {
    let state = app.state::<AppState>();
    let mut changed = false;
    {
        let mut tabs = state.tabs.lock().unwrap();
        if let Some(tab) = tabs.iter_mut().find(|t| t.id == update.tab_id) {
            if let Some(url) = update.url {
                if tab.url != url {
                    tab.url = url;
                    changed = true;
                }
            }
            if let Some(back) = update.can_go_back {
                if tab.can_go_back != back {
                    tab.can_go_back = back;
                    changed = true;
                }
            }
            if let Some(fwd) = update.can_go_forward {
                if tab.can_go_forward != fwd {
                    tab.can_go_forward = fwd;
                    changed = true;
                }
            }
            if let Some(loading) = update.is_loading {
                if tab.is_loading != loading {
                    tab.is_loading = loading;
                    changed = true;
                }
            }
            if let Some(cert_warn) = update.has_cert_warning {
                if tab.has_cert_warning != cert_warn {
                    tab.has_cert_warning = cert_warn;
                    changed = true;
                }
            }
            if let Some(audio) = update.is_playing_audio {
                if tab.is_playing_audio != audio {
                    tab.is_playing_audio = audio;
                    changed = true;
                }
            }
            if let Some(pid) = update.browser_pid {
                if tab.browser_pid != Some(pid) {
                    tab.browser_pid = Some(pid);
                    if let Some(mb) = platform::ACTIVE_CONTROLLER.read_process_memory_mb(pid) {
                        tab.memory_mb = Some(mb);
                    }
                    changed = true;
                }
            }
            if let Some(fav) = update.favicon_url {
                if tab.favicon_url != fav {
                    tab.favicon_url = fav;
                    changed = true;
                }
            }
        }
    }
    if changed {
        emit_shell_state(app);
    }
}

/// Marks a tab (or all tabs sharing a profile when `BrowserProcessExited`) as crashed
/// and refreshes layout so each affected window renders crash recovery without disturbing other tabs.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn apply_process_failed<R: Runtime>(app: &AppHandle<R>, tab_id: &str, kind: TabCrashKind) {
    let state = app.state::<AppState>();
    let affected_windows: Vec<String> = {
        let mut tabs = state.tabs.lock().unwrap();
        let target_prof = tabs
            .iter()
            .find(|t| t.id == tab_id)
            .map(|t| (t.profile_id.clone(), t.is_ephemeral));

        let Some((prof_id, is_eph)) = target_prof else {
            return;
        };

        let mut wins = HashSet::new();
        for tab in tabs.iter_mut() {
            let matches = if kind == TabCrashKind::BrowserProcessExited && !is_eph {
                !tab.is_ephemeral && tab.profile_id == prof_id
            } else {
                tab.id == tab_id
            };
            if matches {
                tab.crash_kind = Some(kind.clone());
                tab.is_loading = false;
                tab.is_sleeping = false;
                tab.is_playing_audio = false;
                wins.insert(tab.window_label.clone());
            }
        }
        wins.into_iter().collect()
    };

    for win in affected_windows {
        sync_bounds_on_resize_for_window(app, &win);
    }
    emit_shell_state(app);
}

/// Removes any leftover per-tab Ephemeral temp directories from prior crashes.
pub fn cleanup_stale_ephemeral_dirs(ephemeral_dir: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(ephemeral_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(p);
            }
        }
    }
}

/// Chooses a startup/fallback profile when opening the first tab or closing the last tab.
/// Prefers explicit default profile in `profiles.json`, then first saved profile, else Ephemeral
/// so no phantom default folders are pre-created.
pub fn default_startup_profile(
    profiles_dir: &std::path::Path,
    profiles_store_path: &std::path::Path,
    legacy_names_path: &std::path::Path,
    legacy_default_key: Option<&str>,
) -> (String, bool) {
    let records = sync_profiles_store(
        profiles_dir,
        profiles_store_path,
        legacy_names_path,
        legacy_default_key,
    );
    if let Some(def) = records.iter().find(|r| r.is_default) {
        return (def.id.clone(), false);
    }
    if let Some(first) = records.into_iter().next() {
        (first.id, false)
    } else {
        ("ephemeral".to_string(), true)
    }
}

/// Computes exact full-window rect in physical pixels (used while an HTML overlay modal is open).
pub fn compute_full_window_rect(window_width: u32, window_height: u32) -> Rect {
    Rect {
        position: PhysicalPosition::new(0_i32, 0_i32).into(),
        size: PhysicalSize::new(window_width.max(320), window_height.max(200)).into(),
    }
}

pub fn current_viewport_rect_for_window<R: Runtime>(
    app: &AppHandle<R>,
    win_label: &str,
) -> Result<Rect, String> {
    let window = app
        .get_window(win_label)
        .ok_or_else(|| format!("Window {win_label} not found"))?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let physical = window.inner_size().map_err(|e| e.to_string())?;
    let state = app.state::<AppState>();
    let win_ui = state.window_state(win_label);
    let density = state.preferences.lock().unwrap().ui_density;
    let shell_h = shell_height_for_prefs(win_ui.find_bar_open, density);
    let (_, _, viewport_rect) = compute_physical_side_layout_rects_with_shell(
        physical.width,
        physical.height,
        scale,
        win_ui.profile_panel_open,
        shell_h,
    );
    Ok(viewport_rect)
}

/// Updates bounds for `ui-shell`, optional drawers, and active content child webview in `win_label`.
pub fn sync_bounds_with_size_for_window<R: Runtime>(
    app: &AppHandle<R>,
    win_label: &str,
    width: u32,
    height: u32,
) {
    if width == 0 || height == 0 {
        return;
    }
    let Some(window) = app.get_window(win_label) else {
        return;
    };
    let Ok(scale) = window.scale_factor() else {
        return;
    };

    let state = app.state::<AppState>();
    let win_ui = state.window_state(win_label);
    let density = state.preferences.lock().unwrap().ui_density;
    let shell_h = shell_height_for_prefs(win_ui.find_bar_open, density);
    let active_id = win_ui.active_tab_id.clone();
    let active_crashed = active_id
        .as_deref()
        .map(|aid| {
            state
                .tabs
                .lock()
                .unwrap()
                .iter()
                .any(|t| t.id == aid && t.crash_kind.is_some())
        })
        .unwrap_or(false);
    let (shell_rect, side_rect, viewport_rect) = compute_physical_side_layout_rects_with_shell(
        width,
        height,
        scale,
        win_ui.profile_panel_open,
        shell_h,
    );

    let shell_lbl = shell_label_for_window(win_label);
    if let Some(shell) = app.get_webview(&shell_lbl) {
        let target_shell = if win_ui.shell_overlay_open || active_crashed {
            compute_full_window_rect(width, height)
        } else {
            shell_rect
        };
        let _ = shell.set_bounds(target_shell);
    }

    let active_wv = active_id.as_deref().and_then(|id| app.get_webview(id));
    let panel_wv = app.get_webview(&panel_label_for_window("profile-panel", win_label));
    let download_wv = app.get_webview(&panel_label_for_window("download-panel", win_label));
    let monitor_wv = app.get_webview(&panel_label_for_window("task-monitor", win_label));

    if win_ui.shell_overlay_open {
        if let Some(pw) = panel_wv {
            let _ = pw.hide();
        }
        if let Some(dw) = download_wv {
            let _ = dw.hide();
        }
        if let Some(mw) = monitor_wv {
            let _ = mw.hide();
        }
        if let Some(wv) = active_wv {
            let _ = wv.hide();
        }
        return;
    }

    if active_crashed {
        if let Some(wv) = active_wv {
            let _ = wv.hide();
        }
    } else if let (tauri::Position::Physical(vp_pos), tauri::Size::Physical(vp_sz)) =
        (viewport_rect.position, viewport_rect.size)
    {
        if let Some(wv) = active_wv {
            let _ = platform::ACTIVE_CONTROLLER.set_bounds_sync(&wv, vp_pos, vp_sz, false, true);
        }
    }

    if let Some(pw) = panel_wv {
        if let Some(sr) = side_rect {
            if let (tauri::Position::Physical(sd_pos), tauri::Size::Physical(sd_sz)) =
                (sr.position, sr.size)
            {
                let _ =
                    platform::ACTIVE_CONTROLLER.set_bounds_sync(&pw, sd_pos, sd_sz, true, true);
            }
        } else {
            let _ = pw.hide();
        }
    }

    for (wv_opt, is_open, logical_w) in [
        (download_wv, win_ui.download_panel_open, DOWNLOAD_PANEL_WIDTH),
        (monitor_wv, win_ui.monitor_panel_open, MONITOR_PANEL_WIDTH),
    ] {
        if let Some(dw) = wv_opt {
            let vis_w = if is_open {
                compute_right_panel_target_width(width, scale, logical_w)
            } else {
                0
            };
            if let Some((dp_pos, dp_sz)) = compute_right_panel_step_rect_with_shell(
                width,
                height,
                scale,
                logical_w,
                vis_w,
                shell_h,
            ) {
                let _ = platform::ACTIVE_CONTROLLER.set_bounds_sync(&dw, dp_pos, dp_sz, true, true);
            } else {
                let _ = dw.hide();
            }
        }
    }
}

fn apply_slide_frame<R: Runtime>(app: &AppHandle<R>, win_label: &str, visible_side_width: u32) {
    let Some(window) = app.get_window(win_label) else {
        return;
    };
    let Ok(scale) = window.scale_factor() else {
        return;
    };
    let Ok(physical) = window.inner_size() else {
        return;
    };
    if physical.width == 0 || physical.height == 0 {
        return;
    }

    let state = app.state::<AppState>();
    let density = state.preferences.lock().unwrap().ui_density;
    let shell_h = shell_height_for_prefs(state.window_state(win_label).find_bar_open, density);
    let (side_opt, _) = compute_slide_step_rects_with_shell(
        physical.width,
        physical.height,
        scale,
        visible_side_width,
        shell_h,
    );
    let panel_lbl = panel_label_for_window("profile-panel", win_label);
    if let (Some(pw), Some((sd_pos, sd_sz))) = (app.get_webview(&panel_lbl), side_opt) {
        let _ = platform::ACTIVE_CONTROLLER.set_bounds_sync(&pw, sd_pos, sd_sz, true, true);
    }
}

/// Opens or closes the floating left Profile Manager side panel for `win_label`.
pub fn set_profile_panel_open_in_window<R: Runtime>(
    app: &AppHandle<R>,
    win_label: &str,
    open: bool,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    state.update_window_state(win_label, |w| {
        w.profile_panel_open = open;
        if open {
            w.download_panel_open = false;
            w.monitor_panel_open = false;
        }
    });
    if open {
        DOWNLOAD_ANIM_GEN.fetch_add(1, Ordering::Relaxed);
        MONITOR_ANIM_GEN.fetch_add(1, Ordering::Relaxed);
    }
    let anim_gen = PANEL_ANIM_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    emit_shell_state(app);

    let window = app
        .get_window(win_label)
        .ok_or_else(|| format!("Window {win_label} not found"))?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let physical = window.inner_size().map_err(|e| e.to_string())?;
    let target_side_w = compute_target_side_width(physical.width, scale);
    let panel_lbl = panel_label_for_window("profile-panel", win_label);
    let density = state.preferences.lock().unwrap().ui_density;
    let base_shell_h = shell_height_for_prefs(state.window_state(win_label).find_bar_open, density);

    if open && app.get_webview(&panel_lbl).is_none() {
        let (_, side_rect, _) = compute_physical_side_layout_rects_with_shell(
            physical.width,
            physical.height,
            scale,
            true,
            base_shell_h,
        );
        if let Some(sr) = side_rect {
            if let tauri::Size::Physical(sd_sz) = sr.size {
                let shell_h = ((base_shell_h * scale).round() as i32).max(1);
                let init_pos = PhysicalPosition::new(-(sd_sz.width as i32), shell_h);
                let builder = WebviewBuilder::new(
                    &panel_lbl,
                    WebviewUrl::App("profile-panel.html".into()),
                );
                let _ = window
                    .add_child(builder, init_pos, sd_sz)
                    .map_err(|e| format!("Gagal membuka side panel profil: {e}"))?;
            }
        }
    }

    const STEPS: u32 = 8;
    const STEP_MS: u64 = 14;
    for step in 1..=STEPS {
        if PANEL_ANIM_GEN.load(Ordering::Relaxed) != anim_gen {
            return Ok(());
        }
        let raw_t = step as f64 / STEPS as f64;
        let eased = 1.0 - (1.0 - raw_t).powi(3);
        let vis_w = if open {
            (target_side_w as f64 * eased).round() as u32
        } else {
            (target_side_w as f64 * (1.0 - eased)).round() as u32
        };
        apply_slide_frame(app, win_label, vis_w);
        if step < STEPS {
            std::thread::sleep(std::time::Duration::from_millis(STEP_MS));
        }
    }

    if PANEL_ANIM_GEN.load(Ordering::Relaxed) != anim_gen {
        return Ok(());
    }

    sync_bounds_on_resize_for_window(app, win_label);
    if open {
        if let Some(panel_wv) = app.get_webview(&panel_lbl) {
            let _ = panel_wv.set_focus();
        }
    } else if let Some(active_id) = state.active_tab_for(win_label) {
        if let Some(wv) = app.get_webview(&active_id) {
            let _ = wv.set_focus();
        }
    }

    Ok(())
}

fn apply_right_drawer_slide_frame<R: Runtime>(
    app: &AppHandle<R>,
    win_label: &str,
    webview_label: &str,
    logical_width: f64,
    visible_width: u32,
) {
    let Some(window) = app.get_window(win_label) else {
        return;
    };
    let Ok(scale) = window.scale_factor() else {
        return;
    };
    let Ok(physical) = window.inner_size() else {
        return;
    };
    if physical.width == 0 || physical.height == 0 {
        return;
    }

    let state = app.state::<AppState>();
    let density = state.preferences.lock().unwrap().ui_density;
    let shell_logical_h =
        shell_height_for_prefs(state.window_state(win_label).find_bar_open, density);
    if let (Some(dw), Some((dp_pos, dp_sz))) = (
        app.get_webview(webview_label),
        compute_right_panel_step_rect_with_shell(
            physical.width,
            physical.height,
            scale,
            logical_width,
            visible_width,
            shell_logical_h,
        ),
    ) {
        let _ = platform::ACTIVE_CONTROLLER.set_bounds_sync(&dw, dp_pos, dp_sz, true, true);
    }
}

fn set_right_drawer_open_in_window<R: Runtime>(
    app: &AppHandle<R>,
    win_label: &str,
    base_label: &str,
    html_file: &str,
    logical_width: f64,
    anim_counter: &AtomicU64,
    open: bool,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let anim_gen = anim_counter.fetch_add(1, Ordering::Relaxed) + 1;
    emit_shell_state(app);

    let window = app
        .get_window(win_label)
        .ok_or_else(|| format!("Window {win_label} not found"))?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let physical = window.inner_size().map_err(|e| e.to_string())?;
    let target_w = compute_right_panel_target_width(physical.width, scale, logical_width);
    let webview_label = panel_label_for_window(base_label, win_label);
    let density = state.preferences.lock().unwrap().ui_density;
    let base_shell_h = shell_height_for_prefs(state.window_state(win_label).find_bar_open, density);

    if open && app.get_webview(&webview_label).is_none() {
        if let Some((_, dp_sz)) = compute_right_panel_step_rect_with_shell(
            physical.width,
            physical.height,
            scale,
            logical_width,
            target_w,
            base_shell_h,
        ) {
            let shell_h = ((base_shell_h * scale).round() as i32).max(1);
            let init_pos = PhysicalPosition::new(physical.width as i32, shell_h);
            let builder =
                WebviewBuilder::new(&webview_label, WebviewUrl::App(html_file.into()));
            let _ = window
                .add_child(builder, init_pos, dp_sz)
                .map_err(|e| format!("Gagal membuka panel {webview_label}: {e}"))?;
        }
    }

    const STEPS: u32 = 8;
    const STEP_MS: u64 = 14;
    for step in 1..=STEPS {
        if anim_counter.load(Ordering::Relaxed) != anim_gen {
            return Ok(());
        }
        let raw_t = step as f64 / STEPS as f64;
        let eased = 1.0 - (1.0 - raw_t).powi(3);
        let vis_w = if open {
            (target_w as f64 * eased).round() as u32
        } else {
            (target_w as f64 * (1.0 - eased)).round() as u32
        };
        apply_right_drawer_slide_frame(app, win_label, &webview_label, logical_width, vis_w);
        if step < STEPS {
            std::thread::sleep(std::time::Duration::from_millis(STEP_MS));
        }
    }

    if anim_counter.load(Ordering::Relaxed) != anim_gen {
        return Ok(());
    }

    sync_bounds_on_resize_for_window(app, win_label);
    if open {
        if let Some(dw) = app.get_webview(&webview_label) {
            let _ = dw.set_focus();
        }
    } else if let Some(active_id) = state.active_tab_for(win_label) {
        if let Some(wv) = app.get_webview(&active_id) {
            let _ = wv.set_focus();
        }
    }

    Ok(())
}

/// Opens or closes the floating right Download drawer for `win_label`.
pub fn set_download_panel_open_in_window<R: Runtime>(
    app: &AppHandle<R>,
    win_label: &str,
    open: bool,
) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        state.update_window_state(win_label, |w| {
            w.download_panel_open = open;
            if open {
                w.profile_panel_open = false;
                w.monitor_panel_open = false;
            }
        });
        if open {
            PANEL_ANIM_GEN.fetch_add(1, Ordering::Relaxed);
            MONITOR_ANIM_GEN.fetch_add(1, Ordering::Relaxed);
        }
    }
    set_right_drawer_open_in_window(
        app,
        win_label,
        "download-panel",
        "download-panel.html",
        DOWNLOAD_PANEL_WIDTH,
        &DOWNLOAD_ANIM_GEN,
        open,
    )
}

/// Opens or closes the floating right Task Monitor & Settings drawer for `win_label`.
pub fn set_monitor_panel_open_in_window<R: Runtime>(
    app: &AppHandle<R>,
    win_label: &str,
    open: bool,
) -> Result<(), String> {
    if open {
        refresh_memory_metrics(app);
    }
    {
        let state = app.state::<AppState>();
        state.update_window_state(win_label, |w| {
            w.monitor_panel_open = open;
            if open {
                w.profile_panel_open = false;
                w.download_panel_open = false;
            }
        });
        if open {
            PANEL_ANIM_GEN.fetch_add(1, Ordering::Relaxed);
            DOWNLOAD_ANIM_GEN.fetch_add(1, Ordering::Relaxed);
        }
    }
    set_right_drawer_open_in_window(
        app,
        win_label,
        "task-monitor",
        "task-monitor.html",
        MONITOR_PANEL_WIDTH,
        &MONITOR_ANIM_GEN,
        open,
    )
}

pub fn sync_bounds_on_resize_for_window<R: Runtime>(app: &AppHandle<R>, win_label: &str) {
    let Some(window) = app.get_window(win_label) else {
        return;
    };
    let Ok(physical) = window.inner_size() else {
        return;
    };
    sync_bounds_with_size_for_window(app, win_label, physical.width, physical.height);
}

/// Updates bounds across all open browser windows.
pub fn sync_bounds_on_resize<R: Runtime>(app: &AppHandle<R>) {
    let win_labels = app.state::<AppState>().window_labels();
    for win_label in win_labels {
        sync_bounds_on_resize_for_window(app, &win_label);
    }
}

/// Shared builder that attaches a child WebView (`tab_id`) to `main` with full RoleTab lifecycle hooks.
#[allow(clippy::too_many_arguments)]
fn attach_child_tab_webview<R: Runtime>(
    window: &tauri::Window<R>,
    app: &AppHandle<R>,
    tab_id: &str,
    parsed_url: &url::Url,
    resolved_profile_id: &str,
    display_name: &str,
    badge_color: &str,
    is_ephemeral: bool,
    data_dir: PathBuf,
    proxy_url: Option<url::Url>,
    zoom_factor: f64,
    viewport: Rect,
) -> Result<tauri::Webview<R>, String> {
    let app_for_title = app.clone();
    let tab_id_for_title = tab_id.to_string();

    let app_for_nav = app.clone();
    let tab_id_for_nav = tab_id.to_string();

    let app_for_new_win = app.clone();
    let tab_id_for_new_win = tab_id.to_string();
    let profile_for_new_win = if is_ephemeral {
        "ephemeral".to_string()
    } else {
        resolved_profile_id.to_string()
    };
    let title_for_popup = display_name.to_string();
    let data_dir_for_popup = data_dir.clone();
    let profile_id_for_popup = resolved_profile_id.to_string();

    let app_for_dl = app.clone();
    let tab_id_for_dl = tab_id.to_string();
    let profile_name_for_dl = display_name.to_string();
    let profile_color_for_dl = badge_color.to_string();

    let mut builder = WebviewBuilder::new(tab_id, WebviewUrl::External(parsed_url.clone()))
        .on_document_title_changed(move |_wv, title| {
            let state = app_for_title.state::<AppState>();
            {
                let mut tabs = state.tabs.lock().unwrap();
                if let Some(tab) = tabs.iter_mut().find(|t| t.id == tab_id_for_title) {
                    tab.title = title;
                }
            }
            emit_shell_state(&app_for_title);
        })
        .on_navigation(move |nav_url| {
            let state = app_for_nav.state::<AppState>();
            {
                let mut tabs = state.tabs.lock().unwrap();
                if let Some(tab) = tabs.iter_mut().find(|t| t.id == tab_id_for_nav) {
                    tab.url = nav_url.to_string();
                }
            }
            emit_shell_state(&app_for_nav);
            true
        })
        .on_new_window(move |req_url, features| {
            if is_popup_features(&features) {
                let popup_id = format!("popup-{}", POPUP_COUNTER.fetch_add(1, Ordering::Relaxed));
                let mut popup_builder = WebviewWindowBuilder::new(
                    &app_for_new_win,
                    &popup_id,
                    WebviewUrl::External("about:blank".parse().unwrap()),
                )
                .window_features(features)
                .title(format!("RoleTab - {title_for_popup}"));
                popup_builder = platform::configure_popup_isolation(
                    popup_builder,
                    is_ephemeral,
                    data_dir_for_popup.clone(),
                    &profile_id_for_popup,
                );

                return match popup_builder.build() {
                    Ok(window) => NewWindowResponse::Create { window },
                    Err(_) => NewWindowResponse::Deny,
                };
            }

            let app_spawn = app_for_new_win.clone();
            let profile_spawn = profile_for_new_win.clone();
            let url_spawn = req_url.to_string();
            let target_win = app_for_new_win
                .state::<AppState>()
                .tab_window_label(&tab_id_for_new_win)
                .unwrap_or_else(|| "main".to_string());
            tauri::async_runtime::spawn_blocking(move || {
                let _ = spawn_tab_in_window(
                    app_spawn,
                    &target_win,
                    profile_spawn,
                    is_ephemeral,
                    Some(url_spawn),
                );
            });
            NewWindowResponse::Deny
        })
        .on_download(move |_wv, event| {
            let state = app_for_dl.state::<AppState>();
            match event {
                DownloadEvent::Requested { url, destination } => {
                    let raw_name = destination
                        .file_name()
                        .and_then(|n| n.to_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .or_else(|| {
                            url.path_segments()
                                .and_then(|segs| segs.last())
                                .filter(|s| !s.is_empty())
                                .map(str::to_string)
                        })
                        .unwrap_or_else(|| "download".to_string());

                    let group_by_profile =
                        state.preferences.lock().unwrap().group_downloads_by_profile;
                    let target_dir = resolve_download_target_dir(
                        &state.downloads_dir,
                        &profile_name_for_dl,
                        group_by_profile,
                    );
                    let _ = std::fs::create_dir_all(&target_dir);
                    let final_path = resolve_unique_download_path(&target_dir, &raw_name);
                    let final_name = final_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("download")
                        .to_string();
                    *destination = final_path.clone();

                    let dl_id = format!(
                        "dl-{}",
                        DOWNLOAD_COUNTER.fetch_add(1, Ordering::Relaxed)
                    );
                    let entry = DownloadEntry {
                        id: dl_id,
                        tab_id: tab_id_for_dl.clone(),
                        profile_name: profile_name_for_dl.clone(),
                        profile_color: profile_color_for_dl.clone(),
                        url: url.to_string(),
                        file_name: final_name,
                        path: final_path.to_string_lossy().to_string(),
                        status: DownloadStatus::InProgress,
                        started_at: unix_now_secs(),
                        finished_at: None,
                    };
                    {
                        let mut downloads = state.downloads.lock().unwrap();
                        downloads.insert(0, entry);
                        downloads.truncate(100);
                    }
                    emit_shell_state(&app_for_dl);
                    true
                }
                DownloadEvent::Finished { url, path, success } => {
                    let url_str = url.to_string();
                    let finished_at = unix_now_secs();
                    let mut updated = false;
                    {
                        let mut downloads = state.downloads.lock().unwrap();
                        let target_idx = downloads
                            .iter()
                            .position(|d| {
                                d.tab_id == tab_id_for_dl
                                    && d.status == DownloadStatus::InProgress
                                    && d.url == url_str
                            })
                            .or_else(|| {
                                downloads.iter().position(|d| {
                                    d.tab_id == tab_id_for_dl
                                        && d.status == DownloadStatus::InProgress
                                })
                            });
                        if let Some(idx) = target_idx {
                            let d = &mut downloads[idx];
                            d.status = if success {
                                DownloadStatus::Completed
                            } else {
                                DownloadStatus::Failed
                            };
                            d.finished_at = Some(finished_at);
                            if let Some(actual_path) = path {
                                if let Some(name) = actual_path.file_name().and_then(|n| n.to_str()) {
                                    d.file_name = name.to_string();
                                }
                                d.path = actual_path.to_string_lossy().to_string();
                            }
                            updated = true;
                        }
                    }
                    if updated {
                        emit_shell_state(&app_for_dl);
                    }
                    true
                }
                _ => true,
            }
        });

    builder = platform::configure_child_isolation(
        builder,
        is_ephemeral,
        data_dir,
        resolved_profile_id,
    );

    if let Some(proxy) = proxy_url {
        builder = builder.proxy_url(proxy);
    }

    let child_webview = window
        .add_child(builder, viewport.position, viewport.size)
        .map_err(|e| format!("Gagal membuat tab: {e}"))?;

    let _ = platform::ACTIVE_CONTROLLER.attach_lifecycle_bridge(
        &child_webview,
        app.clone(),
        tab_id.to_string(),
    );
    if (zoom_factor - 1.0).abs() > 0.001 {
        let _ = platform::ACTIVE_CONTROLLER.set_zoom(&child_webview, zoom_factor);
    }
    let _ = child_webview.set_bounds(viewport);
    Ok(child_webview)
}

/// Spawns a new isolated child webview tab inside `win_label` and focuses it.
pub fn spawn_tab_in_window<R: Runtime>(
    app: AppHandle<R>,
    win_label: &str,
    profile_key: String,
    is_ephemeral: bool,
    target_url: Option<String>,
) -> Result<TabEntry, String> {
    let state = app.state::<AppState>();
    state.ensure_window(win_label);
    let parsed_url = match target_url {
        Some(raw) => normalize_address_input(&raw)?,
        None => DEFAULT_NEW_TAB_URL
            .parse::<url::Url>()
            .map_err(|e| e.to_string())?,
    };

    let tab_id = format!("tab-{}", Uuid::new_v4().simple());
    let (resolved_profile_id, display_name, badge_color, data_dir, proxy_url) = if is_ephemeral {
        let dir = state.ephemeral_dir.join(&tab_id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        (
            tab_id.clone(),
            "Ephemeral".to_string(),
            EPHEMERAL_BADGE_COLOR.to_string(),
            dir,
            None,
        )
    } else {
        let record = if let Some(rec) = find_profile_record(
            &state.profiles_dir,
            &state.profiles_store_path,
            &state.profile_names_path,
            &profile_key,
        ) {
            touch_profile_last_used(&state.profiles_store_path, &rec.id);
            rec
        } else {
            create_profile_record(
                &state.profiles_dir,
                &state.profiles_store_path,
                &state.profile_names_path,
                &profile_key,
                None,
            )?
        };
        let dir = profile_data_dir(&state.profiles_dir, &record.id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let proxy = build_profile_proxy_url(&record.proxy_mode, record.proxy_address.as_deref())?;
        (record.id, record.name, record.color, dir, proxy)
    };

    let viewport = current_viewport_rect_for_window(&app, win_label)?;
    let window = app
        .get_window(win_label)
        .ok_or_else(|| format!("Window {win_label} not found"))?;

    let prev_active = state.active_tab_for(win_label);

    let child_webview = attach_child_tab_webview(
        &window,
        &app,
        &tab_id,
        &parsed_url,
        &resolved_profile_id,
        &display_name,
        &badge_color,
        is_ephemeral,
        data_dir,
        proxy_url,
        1.0,
        viewport,
    )?;

    if state.window_state(win_label).shell_overlay_open {
        let _ = child_webview.hide();
    } else {
        let _ = child_webview.set_focus();
    }

    if let Some(prev_id) = prev_active.as_ref() {
        if let Some(prev_wv) = app.get_webview(prev_id) {
            let _ = prev_wv.hide();
        }
    }

    let now = unix_now_secs();
    let entry = TabEntry {
        id: tab_id.clone(),
        window_label: win_label.to_string(),
        profile_id: resolved_profile_id,
        profile_key: display_name,
        profile_color: badge_color,
        is_ephemeral,
        is_sleeping: false,
        is_pinned: false,
        is_playing_audio: false,
        browser_pid: None,
        memory_mb: None,
        zoom_factor: 1.0,
        crash_kind: None,
        last_active_at: now,
        can_go_back: false,
        can_go_forward: false,
        is_loading: parsed_url.as_str() != DEFAULT_NEW_TAB_URL,
        has_cert_warning: false,
        favicon_url: None,
        title: "Tab Baru".to_string(),
        url: parsed_url.to_string(),
    };

    {
        let mut tabs = state.tabs.lock().unwrap();
        if let Some(prev_id) = prev_active.as_deref() {
            if let Some(prev_tab) = tabs.iter_mut().find(|t| t.id == prev_id) {
                prev_tab.last_active_at = now;
            }
        }
        tabs.push(entry.clone());
    }
    state.set_active_tab_for(win_label, Some(tab_id));

    sync_bounds_on_resize_for_window(&app, win_label);
    emit_shell_state(&app);
    Ok(entry)
}

pub fn spawn_tab<R: Runtime>(
    app: AppHandle<R>,
    profile_key: String,
    is_ephemeral: bool,
    target_url: Option<String>,
) -> Result<TabEntry, String> {
    spawn_tab_in_window(app, "main", profile_key, is_ephemeral, target_url)
}

/// Switches visible child webview to `tab_id` inside its hosting window, resuming if suspended.
pub fn switch_tab_sync<R: Runtime>(app: &AppHandle<R>, tab_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let win_label = state
        .tab_window_label(tab_id)
        .ok_or_else(|| "Tab tidak ditemukan".to_string())?;
    let prev_id = state.active_tab_for(&win_label);
    if prev_id.as_deref() == Some(tab_id) {
        return Ok(());
    }

    let now = unix_now_secs();
    let was_sleeping = {
        let mut tabs = state.tabs.lock().unwrap();
        if let Some(prev_tab_id) = prev_id.as_deref() {
            if let Some(prev_tab) = tabs.iter_mut().find(|t| t.id == prev_tab_id) {
                prev_tab.last_active_at = now;
            }
        }
        let tab = tabs
            .iter_mut()
            .find(|t| t.id == tab_id)
            .ok_or_else(|| "Tab tidak ditemukan".to_string())?;
        let prev = tab.is_sleeping;
        tab.is_sleeping = false;
        tab.last_active_at = now;
        prev
    };

    let viewport = current_viewport_rect_for_window(app, &win_label)?;
    if let Some(wv) = app.get_webview(tab_id) {
        if was_sleeping {
            let _ = platform::ACTIVE_CONTROLLER.resume_tab(&wv);
        }
        let _ = wv.set_bounds(viewport);
        let _ = wv.show();
        let _ = wv.set_focus();
    }

    if let Some(old_id) = prev_id {
        if let Some(old_wv) = app.get_webview(&old_id) {
            let _ = old_wv.hide();
        }
    }

    state.set_active_tab_for(&win_label, Some(tab_id.to_string()));
    sync_bounds_on_resize_for_window(app, &win_label);
    emit_shell_state(app);
    Ok(())
}

/// Recreates a crashed tab (or all tabs of a crashed profile when `BrowserProcessExited`) cleanly in place.
pub fn recover_tab_sync<R: Runtime>(app: AppHandle<R>, tab_id: &str) -> Result<(), String> {
    let targets: Vec<(String, String, String, String, String, bool, String, f64)> = {
        let state = app.state::<AppState>();
        let tabs = state.tabs.lock().unwrap();
        let src = tabs
            .iter()
            .find(|t| t.id == tab_id)
            .ok_or_else(|| "Tab tidak ditemukan".to_string())?;
        let profile_wide =
            src.crash_kind == Some(TabCrashKind::BrowserProcessExited) && !src.is_ephemeral;
        tabs.iter()
            .filter(|t| {
                if profile_wide {
                    !t.is_ephemeral && t.profile_id == src.profile_id && t.crash_kind.is_some()
                } else {
                    t.id == tab_id
                }
            })
            .map(|t| {
                (
                    t.id.clone(),
                    t.window_label.clone(),
                    t.profile_id.clone(),
                    t.profile_key.clone(),
                    t.profile_color.clone(),
                    t.is_ephemeral,
                    t.url.clone(),
                    t.zoom_factor,
                )
            })
            .collect()
    };

    let state = app.state::<AppState>();
    let mut touched_windows = HashSet::new();

    for (old_id, win_label, prof_id, prof_name, prof_color, is_eph, raw_url, zoom) in targets {
        let viewport = current_viewport_rect_for_window(&app, &win_label)?;
        let window = app
            .get_window(&win_label)
            .ok_or_else(|| format!("Window {win_label} not found"))?;

        if let Some(old_wv) = app.get_webview(&old_id) {
            let _ = old_wv.close();
        }
        let parsed_url = normalize_address_input(&raw_url)
            .unwrap_or_else(|_| DEFAULT_NEW_TAB_URL.parse().unwrap());
        let new_id = format!("tab-{}", Uuid::new_v4().simple());
        let (data_dir, proxy_url) = if is_eph {
            let d = state.ephemeral_dir.join(&new_id);
            let _ = std::fs::create_dir_all(&d);
            (d, None)
        } else {
            let d = profile_data_dir(&state.profiles_dir, &prof_id);
            let _ = std::fs::create_dir_all(&d);
            let p_url = find_profile_record(
                &state.profiles_dir,
                &state.profiles_store_path,
                &state.profile_names_path,
                &prof_id,
            )
            .and_then(|rec| {
                build_profile_proxy_url(&rec.proxy_mode, rec.proxy_address.as_deref())
                    .ok()
                    .flatten()
            });
            (d, p_url)
        };

        let win_ui = state.window_state(&win_label);
        let is_active = win_ui.active_tab_id.as_deref() == Some(old_id.as_str());
        let new_wv = attach_child_tab_webview(
            &window,
            &app,
            &new_id,
            &parsed_url,
            &prof_id,
            &prof_name,
            &prof_color,
            is_eph,
            data_dir,
            proxy_url,
            zoom,
            viewport,
        )?;
        if is_active && !win_ui.shell_overlay_open {
            let _ = new_wv.show();
            let _ = new_wv.set_focus();
        } else {
            let _ = new_wv.hide();
        }

        let now = unix_now_secs();
        {
            let mut tabs = state.tabs.lock().unwrap();
            if let Some(tab) = tabs.iter_mut().find(|t| t.id == old_id) {
                tab.id = new_id.clone();
                tab.crash_kind = None;
                tab.is_sleeping = false;
                tab.is_playing_audio = false;
                tab.browser_pid = None;
                tab.memory_mb = None;
                tab.is_loading = parsed_url.as_str() != DEFAULT_NEW_TAB_URL;
                tab.last_active_at = now;
            }
        }
        if is_active {
            state.set_active_tab_for(&win_label, Some(new_id));
        }
        touched_windows.insert(win_label);
    }

    for win in touched_windows {
        sync_bounds_on_resize_for_window(&app, &win);
    }
    emit_shell_state(&app);
    Ok(())
}

/// Toggles the `Keep Awake / Pin` protection on `tab_id`.
pub fn toggle_tab_pin_sync<R: Runtime>(app: &AppHandle<R>, tab_id: &str) -> Result<bool, String> {
    let next = {
        let state = app.state::<AppState>();
        let mut tabs = state.tabs.lock().unwrap();
        let tab = tabs
            .iter_mut()
            .find(|t| t.id == tab_id)
            .ok_or_else(|| "Tab tidak ditemukan".to_string())?;
        tab.is_pinned = !tab.is_pinned;
        tab.is_pinned
    };
    emit_shell_state(app);
    Ok(next)
}

/// Pure helper clamping and rounding per-tab zoom factor (`0.25..=3.0`, step precision 0.01).
pub fn compute_next_zoom(current: f64, delta: Option<f64>) -> f64 {
    match delta {
        None => 1.0,
        Some(d) => ((current + d).clamp(0.25, 3.0) * 100.0).round() / 100.0,
    }
}

/// Adjusts or resets (`delta == None`) zoom level for the active tab in `win_label`.
pub fn adjust_active_tab_zoom_in_window<R: Runtime>(
    app: &AppHandle<R>,
    win_label: &str,
    delta: Option<f64>,
) -> Result<f64, String> {
    let state = app.state::<AppState>();
    let active_id = state
        .active_tab_for(win_label)
        .ok_or_else(|| "Tidak ada tab aktif".to_string())?;
    let next_zoom = {
        let mut tabs = state.tabs.lock().unwrap();
        let tab = tabs
            .iter_mut()
            .find(|t| t.id == active_id)
            .ok_or_else(|| "Tab aktif tidak ditemukan".to_string())?;
        tab.zoom_factor = compute_next_zoom(tab.zoom_factor, delta);
        tab.zoom_factor
    };
    if let Some(wv) = app.get_webview(&active_id) {
        platform::ACTIVE_CONTROLLER.set_zoom(&wv, next_zoom)?;
    }
    emit_shell_state(app);
    Ok(next_zoom)
}

/// Opens or closes the docked Find-in-Page bar in `win_label`'s UI shell.
pub fn set_find_bar_open_in_window<R: Runtime>(
    app: &AppHandle<R>,
    win_label: &str,
    open: bool,
) -> Result<(), String> {
    let active_id = {
        let state = app.state::<AppState>();
        state.update_window_state(win_label, |w| {
            w.find_bar_open = open;
            if !open {
                w.find_query.clear();
            }
        });
        state.active_tab_for(win_label)
    };
    if !open {
        if let Some(id) = active_id.as_deref() {
            if let Some(wv) = app.get_webview(id) {
                let _ = platform::ACTIVE_CONTROLLER.find_in_page(&wv, "", true);
            }
        }
    }
    sync_bounds_on_resize_for_window(app, win_label);
    emit_shell_state(app);
    if open {
        if let Some(shell) = app.get_webview(&shell_label_for_window(win_label)) {
            let _ = shell.set_focus();
        }
    } else if let Some(id) = active_id {
        if let Some(wv) = app.get_webview(&id) {
            let _ = wv.set_focus();
        }
    }
    Ok(())
}

/// Searches `query` inside `win_label`'s active tab DOM without exposing IPC.
pub fn find_in_active_tab_in_window<R: Runtime>(
    app: &AppHandle<R>,
    win_label: &str,
    query: &str,
    forward: bool,
) -> Result<usize, String> {
    let active_id = {
        let state = app.state::<AppState>();
        state.update_window_state(win_label, |w| {
            w.find_query = query.to_string();
        });
        state
            .active_tab_for(win_label)
            .ok_or_else(|| "Tidak ada tab aktif".to_string())?
    };
    if let Some(wv) = app.get_webview(&active_id) {
        platform::ACTIVE_CONTROLLER.find_in_page(&wv, query, forward)
    } else {
        Ok(0)
    }
}

/// Launches every tab in a saved Workspace preset into `win_label` and applies workspace accent theme if set.
pub fn launch_workspace_in_window<R: Runtime>(
    app: AppHandle<R>,
    win_label: &str,
    workspace_id: &str,
) -> Result<usize, String> {
    let preset = {
        let state = app.state::<AppState>();
        let loaded = load_workspaces(&state.workspaces_path)
            .into_iter()
            .find(|w| w.id == workspace_id)
            .ok_or_else(|| "Workspace tidak ditemukan".to_string())?;
        if let Some(ref accent) = loaded.accent_color {
            let mut prefs = state.preferences.lock().unwrap().clone();
            prefs.accent_color = Some(accent.clone());
            let _ = crate::state::save_preferences(&state.prefs_path, &prefs);
            *state.preferences.lock().unwrap() = prefs;
        }
        loaded
    };

    let mut launched = 0_usize;
    for spec in preset.tabs {
        let target_prof = if spec.is_ephemeral {
            "ephemeral".to_string()
        } else {
            spec.profile_id
        };
        let target_url = if spec.url.trim().is_empty() {
            None
        } else {
            Some(spec.url)
        };
        spawn_tab_in_window(app.clone(), win_label, target_prof, spec.is_ephemeral, target_url)?;
        launched += 1;
    }
    emit_shell_state(&app);
    Ok(launched)
}

/// Safely closes tabs using `profile_id` to release WebView2 SQLite locks, exports to
/// `<Downloads>/RoleTab/Exports/<Name>_<epoch>.zip`, and opens Explorer on the archive.
pub fn export_profile_archive_sync<R: Runtime>(
    app: &AppHandle<R>,
    profile_id: &str,
) -> Result<String, String> {
    let (record, profile_dir, open_tab_ids, exports_dir) = {
        let state = app.state::<AppState>();
        let rec = find_profile_record(
            &state.profiles_dir,
            &state.profiles_store_path,
            &state.profile_names_path,
            profile_id,
        )
        .ok_or_else(|| "Profil tidak ditemukan".to_string())?;
        let ids: Vec<String> = state
            .tabs
            .lock()
            .unwrap()
            .iter()
            .filter(|t| !t.is_ephemeral && t.profile_id == rec.id)
            .map(|t| t.id.clone())
            .collect();
        let dir = profile_data_dir(&state.profiles_dir, &rec.id);
        let exp = state.downloads_dir.join("RoleTab").join("Exports");
        (rec, dir, ids, exp)
    };

    let mut became_empty = false;
    for tid in &open_tab_ids {
        became_empty = close_tab_internal(app, tid)?;
    }

    if !open_tab_ids.is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(350));
    }

    let safe_name: String = record
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dest_zip = exports_dir.join(format!("{safe_name}_{}.zip", unix_now_secs()));
    export_profile_dir_to_zip(&record, &profile_dir, &dest_zip)?;

    if became_empty {
        let (fallback_key, is_ephemeral) = {
            let state = app.state::<AppState>();
            let def = state.preferences.lock().unwrap().default_profile.clone();
            default_startup_profile(
                &state.profiles_dir,
                &state.profiles_store_path,
                &state.profile_names_path,
                def.as_deref(),
            )
        };
        let _ = spawn_tab(app.clone(), fallback_key, is_ephemeral, None);
    }

    let _ = reveal_in_explorer(&dest_zip);
    emit_shell_state(app);
    Ok(dest_zip.to_string_lossy().to_string())
}

/// Reads fresh Windows working-set RAM metrics per unique WebView2 `browser_pid`
/// without opening redundant OS process handles.
pub fn refresh_memory_metrics<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    let unique_pids: Vec<u32> = {
        let tabs = state.tabs.lock().unwrap();
        let mut set = HashSet::new();
        for t in tabs.iter() {
            if let Some(pid) = t.browser_pid {
                set.insert(pid);
            }
        }
        set.into_iter().collect()
    };

    if unique_pids.is_empty() {
        return;
    }

    let mut pid_mem = HashMap::new();
    for pid in unique_pids {
        if let Some(mb) = bridge::read_process_working_set_mb(pid) {
            pid_mem.insert(pid, mb);
        }
    }

    let mut changed = false;
    {
        let mut tabs = state.tabs.lock().unwrap();
        for tab in tabs.iter_mut() {
            if let Some(pid) = tab.browser_pid {
                let next = pid_mem.get(&pid).copied();
                if tab.memory_mb != next {
                    tab.memory_mb = next;
                    changed = true;
                }
            }
        }
    }
    if changed {
        emit_shell_state(app);
    }
}

/// Finalizes `is_sleeping` state when WebView2 completes `TrySuspend`.
/// If `TrySuspend` reports `isSuccessful = false`, the tab cleanly remains awake.
pub fn apply_suspend_outcome<R: Runtime>(app: &AppHandle<R>, tab_id: &str, suspended: bool) {
    let state = app.state::<AppState>();
    let changed = {
        let mut tabs = state.tabs.lock().unwrap();
        if let Some(tab) = tabs.iter_mut().find(|t| t.id == tab_id) {
            if tab.is_sleeping != suspended {
                tab.is_sleeping = suspended;
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if changed {
        emit_shell_state(app);
    }
}

/// Suspends a non-active hidden tab via active platform controller.
pub fn sleep_tab_sync<R: Runtime>(app: &AppHandle<R>, tab_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.is_any_active_tab(tab_id) {
        return Err("Tab aktif tidak dapat di-sleep".to_string());
    }

    if let Some(wv) = app.get_webview(tab_id) {
        let _ = wv.hide();
        platform::ACTIVE_CONTROLLER.suspend_tab(&wv, app.clone(), tab_id.to_string())?;
    }

    Ok(())
}

/// Pure selector returning tab IDs eligible for background sleep across all open windows.
#[cfg(test)]
pub fn compute_idle_sleep_candidates(
    tabs: &[TabEntry],
    active_tab_id: Option<&str>,
    downloads: &[DownloadEntry],
    now_secs: u64,
    idle_threshold_secs: Option<u64>,
) -> Vec<String> {
    let active_set: HashSet<&str> = active_tab_id.into_iter().collect();
    compute_idle_sleep_candidates_multi(tabs, &active_set, downloads, now_secs, idle_threshold_secs)
}

pub fn compute_idle_sleep_candidates_multi(
    tabs: &[TabEntry],
    active_ids: &HashSet<&str>,
    downloads: &[DownloadEntry],
    now_secs: u64,
    idle_threshold_secs: Option<u64>,
) -> Vec<String> {
    let Some(threshold) = idle_threshold_secs else {
        return Vec::new();
    };
    let busy_tabs: HashSet<&str> = downloads
        .iter()
        .filter(|d| d.status == DownloadStatus::InProgress)
        .map(|d| d.tab_id.as_str())
        .collect();

    tabs.iter()
        .filter(|tab| {
            !active_ids.contains(tab.id.as_str())
                && !tab.is_sleeping
                && !tab.is_pinned
                && !tab.is_playing_audio
                && tab.crash_kind.is_none()
                && !busy_tabs.contains(tab.id.as_str())
                && now_secs.saturating_sub(tab.last_active_at) >= threshold
        })
        .map(|tab| tab.id.clone())
        .collect()
}

/// Periodic background check: refreshes RAM metrics and sleeps hidden tabs past configured idle duration.
pub fn run_idle_sleep_check<R: Runtime>(app: &AppHandle<R>) {
    refresh_memory_metrics(app);
    let candidates = {
        let state = app.state::<AppState>();
        let idle_mins = state.preferences.lock().unwrap().sleep_idle_minutes;
        let threshold = if idle_mins == 0 {
            None
        } else {
            Some(u64::from(idle_mins) * 60)
        };
        let tabs = state.tabs.lock().unwrap().clone();
        let active_owned: Vec<String> = state
            .windows
            .lock()
            .unwrap()
            .values()
            .filter_map(|w| w.active_tab_id.clone())
            .collect();
        let active_refs: HashSet<&str> = active_owned.iter().map(String::as_str).collect();
        let downloads = state.downloads.lock().unwrap().clone();
        compute_idle_sleep_candidates_multi(&tabs, &active_refs, &downloads, unix_now_secs(), threshold)
    };
    for tab_id in candidates {
        let _ = sleep_tab_sync(app, &tab_id);
    }
}

/// Manually sleeps all inactive background tabs (still protecting pinned, audio, and downloading tabs).
pub fn sleep_inactive_tabs_sync<R: Runtime>(app: &AppHandle<R>) -> Result<usize, String> {
    let candidates = {
        let state = app.state::<AppState>();
        let tabs = state.tabs.lock().unwrap().clone();
        let active_owned: Vec<String> = state
            .windows
            .lock()
            .unwrap()
            .values()
            .filter_map(|w| w.active_tab_id.clone())
            .collect();
        let active_refs: HashSet<&str> = active_owned.iter().map(String::as_str).collect();
        let downloads = state.downloads.lock().unwrap().clone();
        compute_idle_sleep_candidates_multi(&tabs, &active_refs, &downloads, unix_now_secs(), Some(0))
    };
    let count = candidates.len();
    for tab_id in candidates {
        let _ = sleep_tab_sync(app, &tab_id);
    }
    Ok(count)
}

/// Duplicates `source_tab_id`'s current URL into a new tab inside the same hosting window.
pub fn duplicate_tab_sync<R: Runtime>(
    app: AppHandle<R>,
    source_tab_id: &str,
    target_profile: Option<String>,
    target_ephemeral: Option<bool>,
) -> Result<TabEntry, String> {
    let (url, win_label, src_profile, src_ephemeral) = {
        let state = app.state::<AppState>();
        let tabs = state.tabs.lock().unwrap();
        let tab = tabs
            .iter()
            .find(|t| t.id == source_tab_id)
            .ok_or_else(|| "Tab sumber tidak ditemukan".to_string())?;
        (
            tab.url.clone(),
            tab.window_label.clone(),
            tab.profile_id.clone(),
            tab.is_ephemeral,
        )
    };

    let profile_key = target_profile.unwrap_or(src_profile);
    let is_ephemeral = target_ephemeral.unwrap_or(src_ephemeral);
    spawn_tab_in_window(app, &win_label, profile_key, is_ephemeral, Some(url))
}

/// Closes `tab_id` inside its hosting window without auto-respawning when empty.
/// Returns `true` if that window now has zero tabs remaining.
pub fn close_tab_internal<R: Runtime>(app: &AppHandle<R>, tab_id: &str) -> Result<bool, String> {
    let state = app.state::<AppState>();

    let (win_label, next_active, is_win_empty, was_ephemeral) = {
        let mut tabs = state.tabs.lock().unwrap();
        let mut removed_ephemeral = false;
        let mut win = "main".to_string();
        if let Some(i) = tabs.iter().position(|t| t.id == tab_id) {
            removed_ephemeral = tabs[i].is_ephemeral;
            win = tabs[i].window_label.clone();
            tabs.remove(i);
        }
        let next = tabs
            .iter()
            .rev()
            .find(|t| t.window_label == win)
            .map(|t| t.id.clone());
        let empty = !tabs.iter().any(|t| t.window_label == win);
        (win, next, empty, removed_ephemeral)
    };

    let was_active = {
        let old = state.active_tab_for(&win_label);
        let matches = old.as_deref() == Some(tab_id);
        if matches {
            state.set_active_tab_for(&win_label, next_active.clone());
        }
        matches
    };

    if was_active {
        if let Some(next_id) = next_active {
            let viewport = current_viewport_rect_for_window(app, &win_label)?;
            if let Some(wv) = app.get_webview(&next_id) {
                let _ = platform::ACTIVE_CONTROLLER.resume_tab(&wv);
                let _ = wv.set_bounds(viewport);
                let _ = wv.show();
                let _ = wv.set_focus();
            }
            let mut tabs = state.tabs.lock().unwrap();
            if let Some(t) = tabs.iter_mut().find(|t| t.id == next_id) {
                t.is_sleeping = false;
            }
        }
    }

    if let Some(wv) = app.get_webview(tab_id) {
        let _ = wv.close();
    }

    if was_ephemeral {
        let ephem_dir = state.ephemeral_dir.join(tab_id);
        tauri::async_runtime::spawn_blocking(move || {
            let _ = delete_dir_with_retry(&ephem_dir, 10, std::time::Duration::from_millis(120));
        });
    }

    if state.window_state(&win_label).profile_panel_open {
        sync_bounds_on_resize_for_window(app, &win_label);
    }
    emit_shell_state(app);
    Ok(is_win_empty)
}

/// Closes all tabs inside `keep_tab_id`'s window except `keep_tab_id` itself.
pub fn close_other_tabs_sync<R: Runtime>(app: &AppHandle<R>, keep_tab_id: &str) -> Result<(), String> {
    let other_ids: Vec<String> = {
        let state = app.state::<AppState>();
        let tabs = state.tabs.lock().unwrap();
        let keep = tabs
            .iter()
            .find(|t| t.id == keep_tab_id)
            .ok_or_else(|| "Tab target tidak ditemukan".to_string())?;
        let win = keep.window_label.clone();
        tabs.iter()
            .filter(|t| t.id != keep_tab_id && t.window_label == win)
            .map(|t| t.id.clone())
            .collect()
    };

    for id in other_ids {
        let _ = close_tab_internal(app, &id);
    }

    switch_tab_sync(app, keep_tab_id)
}

/// Attaches UI Shell (`ui-shell` / `ui-shell--<win>`) and multi-window event handlers to `window`.
pub fn setup_window_lifecycle<R: Runtime>(
    app: &AppHandle<R>,
    window: &tauri::Window<R>,
) -> Result<(), String> {
    let win_label = window.label().to_string();
    app.state::<AppState>().ensure_window(&win_label);

    let app_ev = app.clone();
    let win_ev = win_label.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(size) => {
            sync_bounds_with_size_for_window(&app_ev, &win_ev, size.width, size.height);
        }
        WindowEvent::ScaleFactorChanged { .. }
        | WindowEvent::Moved(_)
        | WindowEvent::Focused(true) => {
            sync_bounds_on_resize_for_window(&app_ev, &win_ev);
        }
        WindowEvent::CloseRequested { .. } => {
            close_browser_window(&app_ev, &win_ev);
        }
        WindowEvent::Destroyed => {
            let remaining = app_ev.state::<AppState>().remove_window(&win_ev);
            if remaining == 0 {
                app_ev.exit(0);
            } else {
                emit_shell_state(&app_ev);
            }
        }
        _ => {}
    });

    let scale = window.scale_factor().unwrap_or(1.0);
    let inner = window.inner_size().map_err(|e| e.to_string())?;
    let density = app.state::<AppState>().preferences.lock().unwrap().ui_density;
    let shell_h = shell_height_for_prefs(false, density);
    let (shell_rect, _, _) =
        compute_physical_side_layout_rects_with_shell(inner.width, inner.height, scale, false, shell_h);

    let shell_lbl = shell_label_for_window(&win_label);
    let shell_builder = WebviewBuilder::new(&shell_lbl, WebviewUrl::App("index.html".into()));
    window
        .add_child(shell_builder, shell_rect.position, shell_rect.size)
        .map_err(|e| e.to_string())?;
    sync_bounds_on_resize_for_window(app, &win_label);
    let _ = window.show();
    Ok(())
}

/// Closes a browser window's tabs (preserving global persistent profile store) and exits only if last window.
pub fn close_browser_window<R: Runtime>(app: &AppHandle<R>, win_label: &str) {
    let tab_ids: Vec<(String, bool)> = {
        let state = app.state::<AppState>();
        let mut tabs = state.tabs.lock().unwrap();
        let mut removed = Vec::new();
        tabs.retain(|t| {
            if t.window_label == win_label {
                removed.push((t.id.clone(), t.is_ephemeral));
                false
            } else {
                true
            }
        });
        removed
    };

    for (tid, is_eph) in tab_ids {
        if let Some(wv) = app.get_webview(&tid) {
            let _ = wv.close();
        }
        if is_eph {
            let dir = app.state::<AppState>().ephemeral_dir.join(&tid);
            tauri::async_runtime::spawn_blocking(move || {
                let _ = delete_dir_with_retry(&dir, 5, std::time::Duration::from_millis(80));
            });
        }
    }

    let remaining = app.state::<AppState>().remove_window(win_label);
    if remaining == 0 {
        app.exit(0);
    } else {
        emit_shell_state(app);
    }
}

fn window_bg_color<R: Runtime>(app: &AppHandle<R>) -> tauri::window::Color {
    let theme = app.state::<AppState>().preferences.lock().unwrap().theme.clone();
    match theme {
        crate::state::ThemeMode::Dark => tauri::window::Color(32, 33, 36, 255),
        _ => tauri::window::Color(241, 243, 244, 255),
    }
}

/// Spawns a new RoleTab main browser window sharing the same OS process and profile store.
pub fn spawn_browser_window<R: Runtime>(
    app: AppHandle<R>,
    profile_key: Option<String>,
    is_ephemeral: Option<bool>,
    target_url: Option<String>,
    position: Option<(i32, i32)>,
) -> Result<String, String> {
    let win_label = format!("main-{}", WINDOW_COUNTER.fetch_add(1, Ordering::Relaxed));
    let bg = window_bg_color(&app);

    let mut builder = WindowBuilder::new(&app, &win_label)
        .title("RoleTab")
        .inner_size(1280.0, 820.0)
        .min_inner_size(480.0, 320.0)
        .background_color(bg)
        .visible(false);
    if let Some((px, py)) = position {
        builder = builder.position(f64::from(px), f64::from(py));
    }
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone()).map_err(|e| e.to_string())?;
    }

    let window = builder.build().map_err(|e| format!("Gagal membuat window baru: {e}"))?;
    setup_window_lifecycle(&app, &window)?;

    let (resolved_profile, resolved_ephemeral) = if let Some(pk) = profile_key {
        (pk, is_ephemeral.unwrap_or(false))
    } else {
        let state = app.state::<AppState>();
        let def = state.preferences.lock().unwrap().default_profile.clone();
        default_startup_profile(
            &state.profiles_dir,
            &state.profiles_store_path,
            &state.profile_names_path,
            def.as_deref(),
        )
    };

    spawn_tab_in_window(
        app,
        &win_label,
        resolved_profile,
        resolved_ephemeral,
        target_url,
    )?;
    Ok(win_label)
}

/// Pure hit-testing helper returning the first candidate `(label, pos, size)` containing `(screen_x, screen_y)`.
pub fn find_target_window_at_point<'a>(
    candidates: &'a [(String, i32, i32, u32, u32)],
    screen_x: i32,
    screen_y: i32,
) -> Option<&'a str> {
    candidates.iter().find_map(|(label, wx, wy, ww, wh)| {
        let right = wx.saturating_add(*ww as i32);
        let bottom = wy.saturating_add(*wh as i32);
        if screen_x >= *wx && screen_x <= right && screen_y >= *wy && screen_y <= bottom {
            Some(label.as_str())
        } else {
            None
        }
    })
}

/// Moves an existing tab (`tab_id`) cleanly into `target_win_label`, recreating its child webview
/// under the destination window while preserving profile session isolation, URL, zoom, and pin state.
pub fn move_tab_to_window_sync<R: Runtime>(
    app: AppHandle<R>,
    tab_id: &str,
    target_win_label: &str,
) -> Result<(), String> {
    let target_window = app
        .get_window(target_win_label)
        .ok_or_else(|| format!("Window tujuan {target_win_label} tidak ditemukan"))?;

    let state = app.state::<AppState>();
    let (src_win, prof_id, prof_name, prof_color, is_eph, raw_url, zoom, is_pinned, title) = {
        let tabs = state.tabs.lock().unwrap();
        let tab = tabs
            .iter()
            .find(|t| t.id == tab_id)
            .ok_or_else(|| "Tab tidak ditemukan".to_string())?;
        if tab.window_label == target_win_label {
            return Ok(());
        }
        (
            tab.window_label.clone(),
            tab.profile_id.clone(),
            tab.profile_key.clone(),
            tab.profile_color.clone(),
            tab.is_ephemeral,
            tab.url.clone(),
            tab.zoom_factor,
            tab.is_pinned,
            tab.title.clone(),
        )
    };

    let (data_dir, proxy_url) = if is_eph {
        let dir = state.ephemeral_dir.join(tab_id);
        let _ = std::fs::create_dir_all(&dir);
        (dir, None)
    } else {
        let dir = profile_data_dir(&state.profiles_dir, &prof_id);
        let _ = std::fs::create_dir_all(&dir);
        let p_url = find_profile_record(
            &state.profiles_dir,
            &state.profiles_store_path,
            &state.profile_names_path,
            &prof_id,
        )
        .and_then(|rec| {
            build_profile_proxy_url(&rec.proxy_mode, rec.proxy_address.as_deref())
                .ok()
                .flatten()
        });
        (dir, p_url)
    };

    if let Some(old_wv) = app.get_webview(tab_id) {
        let _ = old_wv.close();
    }

    let prev_target_active = state.active_tab_for(target_win_label);
    let viewport = current_viewport_rect_for_window(&app, target_win_label)?;
    let parsed_url =
        normalize_address_input(&raw_url).unwrap_or_else(|_| DEFAULT_NEW_TAB_URL.parse().unwrap());

    let new_wv = attach_child_tab_webview(
        &target_window,
        &app,
        tab_id,
        &parsed_url,
        &prof_id,
        &prof_name,
        &prof_color,
        is_eph,
        data_dir,
        proxy_url,
        zoom,
        viewport,
    )?;

    if let Some(prev_t_id) = prev_target_active.as_ref() {
        if let Some(prev_wv) = app.get_webview(prev_t_id) {
            let _ = prev_wv.hide();
        }
    }

    let (src_next_active, src_empty) = {
        let mut tabs = state.tabs.lock().unwrap();
        if let Some(tab) = tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.window_label = target_win_label.to_string();
            tab.is_sleeping = false;
            tab.is_pinned = is_pinned;
            tab.title = title;
        }
        let next = tabs
            .iter()
            .rev()
            .find(|t| t.window_label == src_win)
            .map(|t| t.id.clone());
        let empty = !tabs.iter().any(|t| t.window_label == src_win);
        (next, empty)
    };

    if state.active_tab_for(&src_win).as_deref() == Some(tab_id) {
        state.set_active_tab_for(&src_win, src_next_active.clone());
        if let Some(next_id) = src_next_active {
            if let Some(wv) = app.get_webview(&next_id) {
                let _ = platform::ACTIVE_CONTROLLER.resume_tab(&wv);
                if let Ok(vp) = current_viewport_rect_for_window(&app, &src_win) {
                    let _ = wv.set_bounds(vp);
                }
                let _ = wv.show();
            }
        }
    }

    state.set_active_tab_for(target_win_label, Some(tab_id.to_string()));
    if !state.window_state(target_win_label).shell_overlay_open {
        let _ = new_wv.show();
        let _ = new_wv.set_focus();
    } else {
        let _ = new_wv.hide();
    }
    let _ = target_window.unminimize();
    let _ = target_window.set_focus();

    if src_empty && state.window_labels().len() > 1 {
        if let Some(src_window) = app.get_window(&src_win) {
            let _ = src_window.close();
        }
    } else if src_empty {
        let def = state.preferences.lock().unwrap().default_profile.clone();
        let (fb_prof, fb_eph) = default_startup_profile(
            &state.profiles_dir,
            &state.profiles_store_path,
            &state.profile_names_path,
            def.as_deref(),
        );
        let _ = spawn_tab_in_window(app.clone(), &src_win, fb_prof, fb_eph, None);
    }

    sync_bounds_on_resize(&app);
    emit_shell_state(&app);
    Ok(())
}

/// Detaches `tab_id` from a multi-tab window into a brand new RoleTab window.
pub fn detach_tab_to_new_window_sync<R: Runtime>(
    app: AppHandle<R>,
    tab_id: &str,
    screen_x: Option<i32>,
    screen_y: Option<i32>,
) -> Result<String, String> {
    let src_win = {
        let state = app.state::<AppState>();
        let tabs = state.tabs.lock().unwrap();
        let tab = tabs
            .iter()
            .find(|t| t.id == tab_id)
            .ok_or_else(|| "Tab tidak ditemukan".to_string())?;
        let win_count = tabs
            .iter()
            .filter(|t| t.window_label == tab.window_label)
            .count();
        if win_count <= 1 {
            return Err("Tab tunggal tidak perlu dipisah ke window baru".to_string());
        }
        tab.window_label.clone()
    };

    let win_label = format!("main-{}", WINDOW_COUNTER.fetch_add(1, Ordering::Relaxed));
    let bg = window_bg_color(&app);
    let mut builder = WindowBuilder::new(&app, &win_label)
        .title("RoleTab")
        .inner_size(1280.0, 820.0)
        .min_inner_size(480.0, 320.0)
        .background_color(bg)
        .visible(false);
    if let (Some(sx), Some(sy)) = (screen_x, screen_y) {
        builder = builder.position(f64::from(sx.saturating_sub(140).max(0)), f64::from(sy.saturating_sub(24).max(0)));
    }
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone()).map_err(|e| e.to_string())?;
    }

    let window = builder.build().map_err(|e| format!("Gagal memisah window: {e}"))?;
    setup_window_lifecycle(&app, &window)?;
    move_tab_to_window_sync(app.clone(), tab_id, &win_label)?;
    let _ = src_win;
    Ok(win_label)
}

/// Drops a dragged tab at `(screen_x, screen_y)`: moves it into an existing RoleTab window under
/// the cursor if found, or detaches it into a new RoleTab window otherwise.
pub fn drop_tab_at_screen_point_sync<R: Runtime>(
    app: AppHandle<R>,
    tab_id: &str,
    screen_x: i32,
    screen_y: i32,
) -> Result<String, String> {
    let state = app.state::<AppState>();
    let src_win = state
        .tab_window_label(tab_id)
        .ok_or_else(|| "Tab tidak ditemukan".to_string())?;

    let candidates: Vec<(String, i32, i32, u32, u32)> = state
        .window_labels()
        .into_iter()
        .filter(|lbl| lbl != &src_win)
        .filter_map(|lbl| {
            let w = app.get_window(&lbl)?;
            let pos = w.outer_position().ok()?;
            let sz = w.outer_size().ok()?;
            Some((lbl, pos.x, pos.y, sz.width, sz.height))
        })
        .collect();

    if let Some(target_win) = find_target_window_at_point(&candidates, screen_x, screen_y) {
        let target = target_win.to_string();
        move_tab_to_window_sync(app, tab_id, &target)?;
        Ok(target)
    } else {
        detach_tab_to_new_window_sync(app, tab_id, Some(screen_x), Some(screen_y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_local_and_private_hosts_accurately() {
        assert!(is_local_or_private_host("localhost:3000/login"));
        assert!(is_local_or_private_host("127.0.0.1:8080"));
        assert!(is_local_or_private_host("192.168.1.50:5173"));
        assert!(is_local_or_private_host("10.0.0.15/admin"));
        assert!(is_local_or_private_host("staging.local"));
        assert!(!is_local_or_private_host("example.com"));
        assert!(!is_local_or_private_host("8.8.8.8"));
    }

    #[test]
    fn normalize_address_input_handles_schemes_and_private_ips() {
        assert_eq!(
            normalize_address_input("about:blank").unwrap().as_str(),
            "about:blank"
        );
        assert_eq!(
            normalize_address_input("localhost:3000/login").unwrap().as_str(),
            "http://localhost:3000/login"
        );
        assert_eq!(
            normalize_address_input("192.168.10.25:8000").unwrap().as_str(),
            "http://192.168.10.25:8000/"
        );
        assert_eq!(
            normalize_address_input("example.com").unwrap().as_str(),
            "https://example.com/"
        );
        assert!(normalize_address_input("   ").is_err());
    }

    #[test]
    fn reorder_tabs_vec_moves_items_and_rejects_invalid_indices() {
        let mut list = vec!["a", "b", "c", "d"];
        assert!(reorder_tabs_vec(&mut list, 0, 2));
        assert_eq!(list, vec!["b", "c", "a", "d"]);
        assert!(!reorder_tabs_vec(&mut list, 1, 1));
        assert!(!reorder_tabs_vec(&mut list, 10, 1));
    }

    #[test]
    fn compute_physical_layout_rects_matches_full_width_across_dpi_scales() {
        for (w, h, scale) in [(1280, 820, 1.0), (1600, 1025, 1.25), (1920, 1080, 1.5)] {
            let (shell_rect, viewport_rect) = compute_physical_layout_rects(w, h, scale);
            match (shell_rect.position, shell_rect.size, viewport_rect.position, viewport_rect.size) {
                (
                    tauri::Position::Physical(s_pos),
                    tauri::Size::Physical(s_sz),
                    tauri::Position::Physical(v_pos),
                    tauri::Size::Physical(v_sz),
                ) => {
                    assert_eq!(s_pos.x, 0);
                    assert_eq!(s_pos.y, 0);
                    assert_eq!(s_sz.width, w);
                    assert_eq!(v_pos.x, 0);
                    assert_eq!(v_pos.y, s_sz.height as i32);
                    assert_eq!(v_sz.width, w);
                    assert_eq!(s_sz.height + v_sz.height, h);
                }
                _ => panic!("expected physical position and size"),
            }
        }
    }

    #[test]
    fn compute_slide_step_rects_interpolates_without_reflow() {
        let target_w = compute_target_side_width(1600, 1.25);
        assert_eq!(target_w, 420);

        let (mid_side, (vp_pos, vp_sz)) = compute_slide_step_rects(1600, 1025, 1.25, 210);
        let (sd_pos, sd_sz) = mid_side.expect("mid-slide must return side rect");
        assert_eq!(sd_pos.x, -210);
        assert_eq!(sd_sz.width, 420);
        assert_eq!(vp_pos.x, 0);
        assert_eq!(vp_sz.width, 1600);

        let (end_side, (end_pos, end_sz)) = compute_slide_step_rects(1600, 1025, 1.25, 0);
        assert!(end_side.is_none());
        assert_eq!(end_pos.x, 0);
        assert_eq!(end_sz.width, 1600);
    }

    #[test]
    fn compute_physical_side_layout_keeps_full_width_viewport_while_floating() {
        for (w, h, scale) in [(1280, 820, 1.0), (1600, 1025, 1.25), (1920, 1080, 1.5)] {
            let (shell_rect, side_opt, viewport_rect) =
                compute_physical_side_layout_rects(w, h, scale, true);
            let side_rect = side_opt.expect("side_rect must exist when side panel is open");
            match (
                shell_rect.size,
                side_rect.position,
                side_rect.size,
                viewport_rect.position,
                viewport_rect.size,
            ) {
                (
                    tauri::Size::Physical(sh_sz),
                    tauri::Position::Physical(sd_pos),
                    tauri::Size::Physical(sd_sz),
                    tauri::Position::Physical(vp_pos),
                    tauri::Size::Physical(vp_sz),
                ) => {
                    assert_eq!(sd_pos.x, 0);
                    assert_eq!(sd_pos.y, sh_sz.height as i32);
                    assert_eq!(vp_pos.x, 0);
                    assert_eq!(vp_pos.y, sh_sz.height as i32);
                    assert_eq!(sd_sz.height, vp_sz.height);
                    assert_eq!(vp_sz.width, w);
                }
                _ => panic!("expected physical floating side layout"),
            }
        }
    }

    #[test]
    fn default_startup_profile_returns_ephemeral_when_empty_and_existing_when_present() {
        let base = std::env::temp_dir().join(format!("roletab_start_{}", Uuid::new_v4().simple()));
        let profiles_dir = base.join("profiles");
        let store_path = base.join("profiles.json");
        let names_path = base.join("profile_names.json");
        std::fs::create_dir_all(&profiles_dir).unwrap();

        assert_eq!(
            default_startup_profile(&profiles_dir, &store_path, &names_path, None),
            ("ephemeral".to_string(), true)
        );

        std::fs::create_dir_all(profiles_dir.join("admin")).unwrap();
        std::fs::create_dir_all(profiles_dir.join("kasir")).unwrap();
        assert_eq!(
            default_startup_profile(&profiles_dir, &store_path, &names_path, None),
            ("admin".to_string(), false)
        );

        let _ = std::fs::remove_file(&store_path);
        assert_eq!(
            default_startup_profile(&profiles_dir, &store_path, &names_path, Some("kasir")),
            ("kasir".to_string(), false)
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cleanup_stale_ephemeral_dirs_removes_leftover_temp_tabs() {
        let base = std::env::temp_dir().join(format!("roletab_eph_{}", Uuid::new_v4().simple()));
        let tab1 = base.join("tab-1");
        let tab2 = base.join("tab-2");
        std::fs::create_dir_all(&tab1).unwrap();
        std::fs::create_dir_all(&tab2).unwrap();
        std::fs::write(tab1.join("cookies"), b"temp").unwrap();

        cleanup_stale_ephemeral_dirs(&base);
        assert!(!tab1.exists());
        assert!(!tab2.exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn compute_download_slide_step_rect_anchors_right_and_respects_height() {
        assert!(
            compute_right_panel_step_rect(1600, 1025, 1.25, DOWNLOAD_PANEL_WIDTH, 0).is_none()
        );

        let (pos, sz) =
            compute_right_panel_step_rect(1600, 1025, 1.25, DOWNLOAD_PANEL_WIDTH, 425)
                .expect("open rect");
        assert_eq!(sz.width, 425);
        assert_eq!(pos.x, 1600 - 425);
        assert_eq!(pos.y, 98);
        assert_eq!(sz.height, 1025 - 98);
    }

    #[test]
    fn compute_idle_sleep_candidates_skips_pinned_audio_crashed_downloading_and_respects_disabled() {
        const IDLE_SECS: u64 = 15 * 60;
        let now = 2_000_u64;
        let mk_tab = |id: &str, sleeping: bool, pinned: bool, audio: bool, crash: Option<TabCrashKind>, last_active_at: u64| TabEntry {
            id: id.into(),
            window_label: "main".into(),
            profile_id: "p1".into(),
            profile_key: "Admin".into(),
            profile_color: "#b91c1c".into(),
            is_ephemeral: false,
            is_sleeping: sleeping,
            is_pinned: pinned,
            is_playing_audio: audio,
            browser_pid: None,
            memory_mb: None,
            zoom_factor: 1.0,
            crash_kind: crash,
            last_active_at,
            can_go_back: false,
            can_go_forward: false,
            is_loading: false,
            has_cert_warning: false,
            favicon_url: None,
            title: id.into(),
            url: "https://example.com".into(),
        };
        let tabs = vec![
            mk_tab("active", false, false, false, None, 0),
            mk_tab("sleeping", true, false, false, None, 0),
            mk_tab("pinned", false, true, false, None, 0),
            mk_tab("audio", false, false, true, None, 0),
            mk_tab("crashed", false, false, false, Some(TabCrashKind::RenderProcessExited), 0),
            mk_tab("recent", false, false, false, None, now - 100),
            mk_tab("downloading", false, false, false, None, 0),
            mk_tab("eligible", false, false, false, None, now - IDLE_SECS - 1),
        ];
        let downloads = vec![DownloadEntry {
            id: "dl-1".into(),
            tab_id: "downloading".into(),
            profile_name: "Admin".into(),
            profile_color: "#b91c1c".into(),
            url: "https://example.com/a.zip".into(),
            file_name: "a.zip".into(),
            path: "C:\\Downloads\\a.zip".into(),
            status: DownloadStatus::InProgress,
            started_at: now - 30,
            finished_at: None,
        }];

        let candidates = compute_idle_sleep_candidates(
            &tabs,
            Some("active"),
            &downloads,
            now,
            Some(IDLE_SECS),
        );
        assert_eq!(candidates, vec!["eligible".to_string()]);

        // When user disables Auto-Sleep (`None`), no background tab should be selected
        assert!(
            compute_idle_sleep_candidates(&tabs, Some("active"), &downloads, now, None).is_empty()
        );
    }

    #[test]
    fn qa_verifies_50_tab_lifecycle_churn_leaves_zero_orphan_ephemeral_dirs_or_tabs() {
        let base =
            std::env::temp_dir().join(format!("roletab_50churn_{}", Uuid::new_v4().simple()));
        let eph_dir = base.join("ephemeral");
        std::fs::create_dir_all(&eph_dir).unwrap();

        let state = AppState::new(
            "131.0.2903.0".into(),
            base.join("profiles"),
            eph_dir.clone(),
            base.join("downloads"),
            base.join("prefs.json"),
            base.join("profiles.json"),
            base.join("names.json"),
        );

        for i in 0..50 {
            let tid = format!("tab-churn-{i}");
            std::fs::create_dir_all(eph_dir.join(&tid)).unwrap();
            {
                let mut tabs = state.tabs.lock().unwrap();
                tabs.push(TabEntry {
                    id: tid.clone(),
                    window_label: "main".into(),
                    profile_id: tid.clone(),
                    profile_key: "Ephemeral".into(),
                    profile_color: "#4b5563".into(),
                    is_ephemeral: true,
                    is_sleeping: false,
                    is_pinned: false,
                    is_playing_audio: false,
                    browser_pid: Some(9000 + (i as u32 % 4)),
                    memory_mb: Some(64),
                    zoom_factor: 1.0,
                    crash_kind: None,
                    last_active_at: i as u64,
                    can_go_back: false,
                    can_go_forward: false,
                    is_loading: false,
                    has_cert_warning: false,
                    favicon_url: None,
                    title: tid.clone(),
                    url: "https://example.test".into(),
                });
                state.set_active_tab_for("main", Some(tid.clone()));
            }
            {
                let mut tabs = state.tabs.lock().unwrap();
                tabs.retain(|t| t.id != tid);
                let next = tabs.last().map(|t| t.id.clone());
                drop(tabs);
                state.set_active_tab_for("main", next);
            }
            delete_dir_with_retry(&eph_dir.join(&tid), 3, std::time::Duration::from_millis(10))
                .unwrap();
        }

        let snap = state.snapshot();
        assert!(snap.tabs.is_empty());
        assert!(snap.active_tab_id.is_none());
        assert_eq!(snap.active_awake_count, 0);
        assert_eq!(snap.total_memory_mb, 0);
        assert_eq!(std::fs::read_dir(&eph_dir).unwrap().count(), 0);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn compute_next_zoom_clamps_rounds_and_resets_cleanly() {
        assert_eq!(compute_next_zoom(1.0, Some(0.1)), 1.1);
        assert_eq!(compute_next_zoom(1.1, Some(0.1)), 1.2);
        assert_eq!(compute_next_zoom(2.95, Some(0.2)), 3.0);
        assert_eq!(compute_next_zoom(0.3, Some(-0.2)), 0.25);
        assert_eq!(compute_next_zoom(1.4, None), 1.0);
        assert_eq!(shell_height_for_find(false), UI_SHELL_HEIGHT);
        assert_eq!(shell_height_for_find(true), UI_SHELL_FIND_HEIGHT);
        assert_eq!(shell_height_for_prefs(false, UiDensity::Compact), UI_SHELL_COMPACT_HEIGHT);
        assert_eq!(shell_height_for_prefs(true, UiDensity::Compact), UI_SHELL_COMPACT_FIND_HEIGHT);
    }

    #[test]
    fn resolves_multi_window_labels_reordering_and_screen_hit_testing() {
        assert_eq!(shell_label_for_window("main"), "ui-shell");
        assert_eq!(shell_label_for_window("main-2"), "ui-shell--main-2");
        assert_eq!(window_label_from_webview_label("ui-shell"), "main");
        assert_eq!(window_label_from_webview_label("ui-shell--main-2"), "main-2");
        assert_eq!(window_label_from_webview_label("task-monitor--main-3"), "main-3");

        let mk = |id: &str, win: &str| TabEntry {
            id: id.into(),
            window_label: win.into(),
            profile_id: "p1".into(),
            profile_key: "Admin".into(),
            profile_color: "#b91c1c".into(),
            is_ephemeral: false,
            is_sleeping: false,
            is_pinned: false,
            is_playing_audio: false,
            browser_pid: None,
            memory_mb: None,
            zoom_factor: 1.0,
            crash_kind: None,
            last_active_at: 0,
            can_go_back: false,
            can_go_forward: false,
            is_loading: false,
            has_cert_warning: false,
            favicon_url: None,
            title: id.into(),
            url: "https://example.com".into(),
        };

        let mut tabs = vec![
            mk("a1", "main"),
            mk("b1", "main-2"),
            mk("a2", "main"),
            mk("a3", "main"),
        ];
        assert!(reorder_window_tabs_vec(&mut tabs, "main", 0, 2));
        let main_ids: Vec<&str> = tabs
            .iter()
            .filter(|t| t.window_label == "main")
            .map(|t| t.id.as_str())
            .collect();
        let w2_ids: Vec<&str> = tabs
            .iter()
            .filter(|t| t.window_label == "main-2")
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(main_ids, vec!["a2", "a3", "a1"]);
        assert_eq!(w2_ids, vec!["b1"]);

        let wins = vec![
            ("main-2".to_string(), 1400, 100, 1280, 820),
            ("main-3".to_string(), 50, 50, 800, 600),
        ];
        assert_eq!(find_target_window_at_point(&wins, 1600, 300), Some("main-2"));
        assert_eq!(find_target_window_at_point(&wins, 200, 200), Some("main-3"));
        assert_eq!(find_target_window_at_point(&wins, 1300, 300), None);
    }
}
