pub mod bridge;

use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::webview::{DownloadEvent, NewWindowFeatures, NewWindowResponse, WebviewBuilder};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Rect, Runtime, WebviewUrl,
    WebviewWindowBuilder,
};
use uuid::Uuid;

use crate::downloads::{resolve_download_target_dir, resolve_unique_download_path};
use crate::profile::{
    create_profile_record, delete_dir_with_retry, find_profile_record, profile_data_dir,
    sync_profiles_store, touch_profile_last_used, EPHEMERAL_BADGE_COLOR,
};
use crate::state::{AppState, DownloadEntry, DownloadStatus, TabEntry};
use bridge::NativeNavState;

/// Compact 2-row Chrome-like UI Shell height (36px Tab Strip + 42px Navigation Toolbar).
pub const UI_SHELL_HEIGHT: f64 = 78.0;

/// Logical width of the Profile Manager side panel when open.
pub const PROFILE_PANEL_WIDTH: f64 = 336.0;

/// Logical width of the Download Manager floating drawer when open.
pub const DOWNLOAD_PANEL_WIDTH: f64 = 340.0;

/// Idle threshold (in seconds) before a non-active background tab is eligible for auto-sleep.
pub const IDLE_SLEEP_SECONDS: u64 = 15 * 60;

/// Default startup / Home URL for tabs.
pub const DEFAULT_NEW_TAB_URL: &str = "about:blank";

static POPUP_COUNTER: AtomicU64 = AtomicU64::new(1);
static PANEL_ANIM_GEN: AtomicU64 = AtomicU64::new(1);
static DOWNLOAD_ANIM_GEN: AtomicU64 = AtomicU64::new(1);
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

/// Computes exact `(side_pos, side_size, viewport_pos, viewport_size)` in physical pixels for any
/// intermediate `visible_side_width` during slide-open / slide-close animations.
/// Because the side panel floats above the viewport, the browser viewport ALWAYS remains full width.
pub fn compute_slide_step_rects(
    window_width: u32,
    window_height: u32,
    scale_factor: f64,
    visible_side_width: u32,
) -> (
    Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>,
    (PhysicalPosition<i32>, PhysicalSize<u32>),
) {
    let scale = if scale_factor > 0.0 { scale_factor } else { 1.0 };
    let width = window_width.max(320);
    let shell_height =
        ((UI_SHELL_HEIGHT * scale).round() as u32).min(window_height.saturating_sub(80).max(1));
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

/// Computes exact physical pixel `Rect`s for `(ui_shell, optional_side_panel, content_viewport)`
/// from the parent window's physical `inner_size` and `scale_factor`.
pub fn compute_physical_side_layout_rects(
    window_width: u32,
    window_height: u32,
    scale_factor: f64,
    side_open: bool,
) -> (Rect, Option<Rect>, Rect) {
    let scale = if scale_factor > 0.0 { scale_factor } else { 1.0 };
    let width = window_width.max(320);
    let shell_height =
        ((UI_SHELL_HEIGHT * scale).round() as u32).min(window_height.saturating_sub(80).max(1));
    let target_side_w = if side_open {
        compute_target_side_width(width, scale)
    } else {
        0
    };
    let (side_opt, (vp_pos, vp_sz)) =
        compute_slide_step_rects(width, window_height, scale, target_side_w);

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

/// Computes the target physical width for the right Download drawer when fully open.
pub fn compute_target_download_width(window_width: u32, scale_factor: f64) -> u32 {
    let scale = if scale_factor > 0.0 { scale_factor } else { 1.0 };
    let width = window_width.max(320);
    let min_viewport = (260.0 * scale).round() as u32;
    let max_side = width.saturating_sub(min_viewport).max(160);
    ((DOWNLOAD_PANEL_WIDTH * scale).round() as u32)
        .min(max_side)
        .max(180)
}

/// Computes `(pos, size)` in physical pixels for the right floating Download drawer
/// at intermediate `visible_width` during slide-open / slide-close animation.
pub fn compute_download_slide_step_rect(
    window_width: u32,
    window_height: u32,
    scale_factor: f64,
    visible_width: u32,
) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    let scale = if scale_factor > 0.0 { scale_factor } else { 1.0 };
    let width = window_width.max(320);
    let shell_height =
        ((UI_SHELL_HEIGHT * scale).round() as u32).min(window_height.saturating_sub(80).max(1));
    let content_height = window_height.saturating_sub(shell_height).max(1);
    let target_w = compute_target_download_width(width, scale);
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

/// Computes exact physical pixel `Rect`s for `(ui_shell, content_viewport)` from the parent window's
/// physical `inner_size` and `scale_factor`, avoiding logical/physical DPI round-trip mismatches in WebView2.
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

/// Reorders a slice of tabs safely when dragging a tab from `from_idx` to `to_idx`.
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
    let snapshot = state.snapshot();
    let _ = app.emit_to("ui-shell", "shell-state", &snapshot);
    if app.get_webview("profile-panel").is_some() {
        let _ = app.emit_to("profile-panel", "shell-state", &snapshot);
    }
    if app.get_webview("download-panel").is_some() {
        let _ = app.emit_to("download-panel", "shell-state", &snapshot);
    }
}

/// Applies native WebView2 navigation/favicon/certificate updates into `AppState`.
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

pub fn current_viewport_rect<R: Runtime>(app: &AppHandle<R>) -> Result<Rect, String> {
    let window = app
        .get_window("main")
        .ok_or_else(|| "Main window not found".to_string())?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let physical = window.inner_size().map_err(|e| e.to_string())?;
    let side_open = *app.state::<AppState>().profile_panel_open.lock().unwrap();
    let (_, _, viewport_rect) =
        compute_physical_side_layout_rects(physical.width, physical.height, scale, side_open);
    Ok(viewport_rect)
}

/// Updates bounds for `ui-shell`, optional `profile-panel`, and active content child webview.
pub fn sync_bounds_with_size<R: Runtime>(app: &AppHandle<R>, width: u32, height: u32) {
    if width == 0 || height == 0 {
        return;
    }
    let Some(window) = app.get_window("main") else {
        return;
    };
    let Ok(scale) = window.scale_factor() else {
        return;
    };

    let state = app.state::<AppState>();
    let overlay_open = *state.shell_overlay_open.lock().unwrap();
    let side_open = *state.profile_panel_open.lock().unwrap();
    let download_open = *state.download_panel_open.lock().unwrap();
    let (shell_rect, side_rect, viewport_rect) =
        compute_physical_side_layout_rects(width, height, scale, side_open);

    if let Some(shell) = app.get_webview("ui-shell") {
        let target_shell = if overlay_open {
            compute_full_window_rect(width, height)
        } else {
            shell_rect
        };
        let _ = shell.set_bounds(target_shell);
    }

    let active_id = state.active_tab_id.lock().unwrap().clone();
    let active_wv = active_id.as_deref().and_then(|id| app.get_webview(id));
    let panel_wv = app.get_webview("profile-panel");
    let download_wv = app.get_webview("download-panel");

    if overlay_open {
        if let Some(pw) = panel_wv {
            let _ = pw.hide();
        }
        if let Some(dw) = download_wv {
            let _ = dw.hide();
        }
        if let Some(wv) = active_wv {
            let _ = wv.hide();
        }
        return;
    }

    if let (tauri::Position::Physical(vp_pos), tauri::Size::Physical(vp_sz)) =
        (viewport_rect.position, viewport_rect.size)
    {
        if let Some(wv) = active_wv {
            let _ = bridge::set_bounds_sync_native(&wv, vp_pos, vp_sz, false, true);
        }
    }

    if let Some(pw) = panel_wv {
        if let Some(sr) = side_rect {
            if let (tauri::Position::Physical(sd_pos), tauri::Size::Physical(sd_sz)) =
                (sr.position, sr.size)
            {
                let _ = bridge::set_bounds_sync_native(&pw, sd_pos, sd_sz, true, true);
            }
        } else {
            let _ = pw.hide();
        }
    }

    if let Some(dw) = download_wv {
        let dl_vis_w = if download_open {
            compute_target_download_width(width, scale)
        } else {
            0
        };
        if let Some((dp_pos, dp_sz)) =
            compute_download_slide_step_rect(width, height, scale, dl_vis_w)
        {
            let _ = bridge::set_bounds_sync_native(&dw, dp_pos, dp_sz, true, true);
        } else {
            let _ = dw.hide();
        }
    }
}

fn apply_slide_frame<R: Runtime>(app: &AppHandle<R>, visible_side_width: u32) {
    let Some(window) = app.get_window("main") else {
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

    let (side_opt, _) =
        compute_slide_step_rects(physical.width, physical.height, scale, visible_side_width);
    if let (Some(pw), Some((sd_pos, sd_sz))) = (app.get_webview("profile-panel"), side_opt) {
        let _ = bridge::set_bounds_sync_native(&pw, sd_pos, sd_sz, true, true);
    }
}

/// Opens or closes the floating left Profile Manager side panel (`profile-panel` child webview)
/// with a smooth cubic ease-out slide animation above the full-width browser viewport.
pub fn set_profile_panel_open_sync<R: Runtime>(
    app: &AppHandle<R>,
    open: bool,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    *state.profile_panel_open.lock().unwrap() = open;
    if open {
        *state.download_panel_open.lock().unwrap() = false;
        DOWNLOAD_ANIM_GEN.fetch_add(1, Ordering::Relaxed);
    }
    let anim_gen = PANEL_ANIM_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    emit_shell_state(app);

    let window = app
        .get_window("main")
        .ok_or_else(|| "Main window not found".to_string())?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let physical = window.inner_size().map_err(|e| e.to_string())?;
    let target_side_w = compute_target_side_width(physical.width, scale);

    if open && app.get_webview("profile-panel").is_none() {
        let (_, side_rect, _) =
            compute_physical_side_layout_rects(physical.width, physical.height, scale, true);
        if let Some(sr) = side_rect {
            if let tauri::Size::Physical(sd_sz) = sr.size {
                let shell_h = ((UI_SHELL_HEIGHT * scale).round() as i32).max(1);
                let init_pos = PhysicalPosition::new(-(sd_sz.width as i32), shell_h);
                let builder = WebviewBuilder::new(
                    "profile-panel",
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
        apply_slide_frame(app, vis_w);
        if step < STEPS {
            std::thread::sleep(std::time::Duration::from_millis(STEP_MS));
        }
    }

    if PANEL_ANIM_GEN.load(Ordering::Relaxed) != anim_gen {
        return Ok(());
    }

    sync_bounds_on_resize(app);
    if open {
        if let Some(panel_wv) = app.get_webview("profile-panel") {
            let _ = panel_wv.set_focus();
        }
    } else if let Some(active_id) = state.active_tab_id.lock().unwrap().clone() {
        if let Some(wv) = app.get_webview(&active_id) {
            let _ = wv.set_focus();
        }
    }

    Ok(())
}

fn apply_download_slide_frame<R: Runtime>(app: &AppHandle<R>, visible_width: u32) {
    let Some(window) = app.get_window("main") else {
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

    if let (Some(dw), Some((dp_pos, dp_sz))) = (
        app.get_webview("download-panel"),
        compute_download_slide_step_rect(physical.width, physical.height, scale, visible_width),
    ) {
        let _ = bridge::set_bounds_sync_native(&dw, dp_pos, dp_sz, true, true);
    }
}

/// Opens or closes the floating right Download drawer (`download-panel` child webview)
/// with a smooth cubic ease-out slide animation above the full-width browser viewport.
pub fn set_download_panel_open_sync<R: Runtime>(
    app: &AppHandle<R>,
    open: bool,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    *state.download_panel_open.lock().unwrap() = open;
    if open {
        *state.profile_panel_open.lock().unwrap() = false;
        PANEL_ANIM_GEN.fetch_add(1, Ordering::Relaxed);
    }
    let anim_gen = DOWNLOAD_ANIM_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    emit_shell_state(app);

    let window = app
        .get_window("main")
        .ok_or_else(|| "Main window not found".to_string())?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let physical = window.inner_size().map_err(|e| e.to_string())?;
    let target_w = compute_target_download_width(physical.width, scale);

    if open && app.get_webview("download-panel").is_none() {
        if let Some((_, dp_sz)) =
            compute_download_slide_step_rect(physical.width, physical.height, scale, target_w)
        {
            let shell_h = ((UI_SHELL_HEIGHT * scale).round() as i32).max(1);
            let init_pos = PhysicalPosition::new(physical.width as i32, shell_h);
            let builder = WebviewBuilder::new(
                "download-panel",
                WebviewUrl::App("download-panel.html".into()),
            );
            let _ = window
                .add_child(builder, init_pos, dp_sz)
                .map_err(|e| format!("Gagal membuka panel unduhan: {e}"))?;
        }
    }

    const STEPS: u32 = 8;
    const STEP_MS: u64 = 14;
    for step in 1..=STEPS {
        if DOWNLOAD_ANIM_GEN.load(Ordering::Relaxed) != anim_gen {
            return Ok(());
        }
        let raw_t = step as f64 / STEPS as f64;
        let eased = 1.0 - (1.0 - raw_t).powi(3);
        let vis_w = if open {
            (target_w as f64 * eased).round() as u32
        } else {
            (target_w as f64 * (1.0 - eased)).round() as u32
        };
        apply_download_slide_frame(app, vis_w);
        if step < STEPS {
            std::thread::sleep(std::time::Duration::from_millis(STEP_MS));
        }
    }

    if DOWNLOAD_ANIM_GEN.load(Ordering::Relaxed) != anim_gen {
        return Ok(());
    }

    sync_bounds_on_resize(app);
    if open {
        if let Some(dw) = app.get_webview("download-panel") {
            let _ = dw.set_focus();
        }
    } else if let Some(active_id) = state.active_tab_id.lock().unwrap().clone() {
        if let Some(wv) = app.get_webview(&active_id) {
            let _ = wv.set_focus();
        }
    }

    Ok(())
}

/// Updates bounds for `ui-shell` and the active content child webview on window resize/DPI change.
pub fn sync_bounds_on_resize<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_window("main") else {
        return;
    };
    let Ok(physical) = window.inner_size() else {
        return;
    };
    sync_bounds_with_size(app, physical.width, physical.height);
}

/// Spawns a new isolated child webview tab and focuses it.
/// Must be called off the main UI event-loop thread to avoid WebView2 `add_child` deadlocks.
pub fn spawn_tab<R: Runtime>(
    app: AppHandle<R>,
    profile_key: String,
    is_ephemeral: bool,
    target_url: Option<String>,
) -> Result<TabEntry, String> {
    let state = app.state::<AppState>();
    let parsed_url = match target_url {
        Some(raw) => normalize_address_input(&raw)?,
        None => DEFAULT_NEW_TAB_URL
            .parse::<url::Url>()
            .map_err(|e| e.to_string())?,
    };

    let tab_id = format!("tab-{}", Uuid::new_v4().simple());
    let (resolved_profile_id, display_name, badge_color, data_dir) = if is_ephemeral {
        let dir = state.ephemeral_dir.join(&tab_id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        (
            tab_id.clone(),
            "Ephemeral".to_string(),
            EPHEMERAL_BADGE_COLOR.to_string(),
            dir,
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
        (record.id, record.name, record.color, dir)
    };

    let viewport = current_viewport_rect(&app)?;
    let window = app
        .get_window("main")
        .ok_or_else(|| "Main window not found".to_string())?;

    let prev_active = state.active_tab_id.lock().unwrap().clone();

    let app_for_title = app.clone();
    let tab_id_for_title = tab_id.clone();

    let app_for_nav = app.clone();
    let tab_id_for_nav = tab_id.clone();

    let app_for_new_win = app.clone();
    let profile_for_new_win = if is_ephemeral {
        "ephemeral".to_string()
    } else {
        resolved_profile_id.clone()
    };
    let title_for_popup = display_name.clone();
    let data_dir_for_popup = data_dir.clone();

    let app_for_dl = app.clone();
    let tab_id_for_dl = tab_id.clone();
    let profile_name_for_dl = display_name.clone();
    let profile_color_for_dl = badge_color.clone();

    let mut builder = WebviewBuilder::new(&tab_id, WebviewUrl::External(parsed_url.clone()))
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
                .title(format!("RoleTab - {title_for_popup}"))
                .data_directory(data_dir_for_popup.clone());

                if is_ephemeral {
                    popup_builder = popup_builder.incognito(true);
                }

                return match popup_builder.build() {
                    Ok(window) => NewWindowResponse::Create { window },
                    Err(_) => NewWindowResponse::Deny,
                };
            }

            let app_spawn = app_for_new_win.clone();
            let profile_spawn = profile_for_new_win.clone();
            let url_spawn = req_url.to_string();
            tauri::async_runtime::spawn_blocking(move || {
                let _ = spawn_tab(app_spawn, profile_spawn, is_ephemeral, Some(url_spawn));
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
        })
        .data_directory(data_dir);

    if is_ephemeral {
        builder = builder.incognito(true);
    }

    let child_webview = window
        .add_child(builder, viewport.position, viewport.size)
        .map_err(|e| format!("Gagal membuat tab: {e}"))?;

    let _ = bridge::attach_native_hooks(&child_webview, app.clone(), tab_id.clone());
    let _ = child_webview.set_bounds(viewport);
    if *state.shell_overlay_open.lock().unwrap() {
        let _ = child_webview.hide();
    } else {
        let _ = child_webview.set_focus();
    }

    // Hide the previous tab ONLY AFTER the new child webview exists and is positioned,
    // eliminating the black parent-window flash during tab creation.
    if let Some(prev_id) = prev_active.as_ref() {
        if let Some(prev_wv) = app.get_webview(prev_id) {
            let _ = prev_wv.hide();
        }
    }

    let now = unix_now_secs();
    let entry = TabEntry {
        id: tab_id.clone(),
        profile_id: resolved_profile_id,
        profile_key: display_name,
        profile_color: badge_color,
        is_ephemeral,
        is_sleeping: false,
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
    {
        let mut active = state.active_tab_id.lock().unwrap();
        *active = Some(tab_id);
    }

    if *state.profile_panel_open.lock().unwrap() {
        sync_bounds_on_resize(&app);
    }
    emit_shell_state(&app);
    Ok(entry)
}

/// Switches visible child webview to `tab_id`, resuming it if suspended.
pub fn switch_tab_sync<R: Runtime>(app: &AppHandle<R>, tab_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let prev_id = state.active_tab_id.lock().unwrap().clone();
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

    let viewport = current_viewport_rect(app)?;
    if let Some(wv) = app.get_webview(tab_id) {
        if was_sleeping {
            let _ = bridge::resume_webview(&wv);
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

    *state.active_tab_id.lock().unwrap() = Some(tab_id.to_string());
    if *state.profile_panel_open.lock().unwrap() {
        sync_bounds_on_resize(app);
    }
    emit_shell_state(app);
    Ok(())
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

/// Suspends a non-active hidden tab via `ICoreWebView2_3::TrySuspend`.
pub fn sleep_tab_sync<R: Runtime>(app: &AppHandle<R>, tab_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let active_id = state.active_tab_id.lock().unwrap().clone();
    if active_id.as_deref() == Some(tab_id) {
        return Err("Tab aktif tidak dapat di-sleep".to_string());
    }

    if let Some(wv) = app.get_webview(tab_id) {
        let _ = wv.hide();
        bridge::suspend_webview(&wv, app.clone(), tab_id.to_string())?;
    }

    Ok(())
}

/// Pure selector returning tab IDs eligible for background sleep.
/// Excludes active tab, already sleeping tabs, tabs with active in-progress downloads,
/// and tabs whose `last_active_at` is newer than `idle_threshold_secs`.
pub fn compute_idle_sleep_candidates(
    tabs: &[TabEntry],
    active_tab_id: Option<&str>,
    downloads: &[DownloadEntry],
    now_secs: u64,
    idle_threshold_secs: u64,
) -> Vec<String> {
    let busy_tabs: HashSet<&str> = downloads
        .iter()
        .filter(|d| d.status == DownloadStatus::InProgress)
        .map(|d| d.tab_id.as_str())
        .collect();

    tabs.iter()
        .filter(|tab| {
            Some(tab.id.as_str()) != active_tab_id
                && !tab.is_sleeping
                && !busy_tabs.contains(tab.id.as_str())
                && now_secs.saturating_sub(tab.last_active_at) >= idle_threshold_secs
        })
        .map(|tab| tab.id.clone())
        .collect()
}

/// Periodic background check: suspends hidden tabs idle >= 15 minutes without active downloads.
pub fn run_idle_sleep_check<R: Runtime>(app: &AppHandle<R>) {
    let candidates = {
        let state = app.state::<AppState>();
        let active_id = state.active_tab_id.lock().unwrap().clone();
        let tabs = state.tabs.lock().unwrap().clone();
        let downloads = state.downloads.lock().unwrap().clone();
        compute_idle_sleep_candidates(
            &tabs,
            active_id.as_deref(),
            &downloads,
            unix_now_secs(),
            IDLE_SLEEP_SECONDS,
        )
    };
    for tab_id in candidates {
        let _ = sleep_tab_sync(app, &tab_id);
    }
}

/// Manually sleeps all inactive background tabs (used by the high awake-tab warning pill).
pub fn sleep_inactive_tabs_sync<R: Runtime>(app: &AppHandle<R>) -> Result<usize, String> {
    let candidates = {
        let state = app.state::<AppState>();
        let active_id = state.active_tab_id.lock().unwrap().clone();
        let tabs = state.tabs.lock().unwrap().clone();
        let downloads = state.downloads.lock().unwrap().clone();
        compute_idle_sleep_candidates(&tabs, active_id.as_deref(), &downloads, unix_now_secs(), 0)
    };
    let count = candidates.len();
    for tab_id in candidates {
        let _ = sleep_tab_sync(app, &tab_id);
    }
    Ok(count)
}

/// Duplicates `source_tab_id`'s current URL into a new tab using the target profile or ephemeral mode.
pub fn duplicate_tab_sync<R: Runtime>(
    app: AppHandle<R>,
    source_tab_id: &str,
    target_profile: Option<String>,
    target_ephemeral: Option<bool>,
) -> Result<TabEntry, String> {
    let (url, src_profile, src_ephemeral) = {
        let state = app.state::<AppState>();
        let tabs = state.tabs.lock().unwrap();
        let tab = tabs
            .iter()
            .find(|t| t.id == source_tab_id)
            .ok_or_else(|| "Tab sumber tidak ditemukan".to_string())?;
        (tab.url.clone(), tab.profile_id.clone(), tab.is_ephemeral)
    };

    let profile_key = target_profile.unwrap_or(src_profile);
    let is_ephemeral = target_ephemeral.unwrap_or(src_ephemeral);
    spawn_tab(app, profile_key, is_ephemeral, Some(url))
}

/// Closes a tab without auto-respawning when empty (used internally by profile deletion).
pub fn close_tab_internal<R: Runtime>(app: &AppHandle<R>, tab_id: &str) -> Result<bool, String> {
    let state = app.state::<AppState>();

    let (next_active, is_empty, was_ephemeral) = {
        let mut tabs = state.tabs.lock().unwrap();
        let mut removed_ephemeral = false;
        if let Some(i) = tabs.iter().position(|t| t.id == tab_id) {
            removed_ephemeral = tabs[i].is_ephemeral;
            tabs.remove(i);
        }
        (
            tabs.last().map(|t| t.id.clone()),
            tabs.is_empty(),
            removed_ephemeral,
        )
    };

    let was_active = {
        let mut active = state.active_tab_id.lock().unwrap();
        let matches = active.as_deref() == Some(tab_id);
        if matches {
            *active = next_active.clone();
        }
        matches
    };

    // Show next active tab BEFORE closing the outgoing webview so no black gap appears.
    if was_active {
        if let Some(next_id) = next_active {
            let viewport = current_viewport_rect(app)?;
            if let Some(wv) = app.get_webview(&next_id) {
                let _ = bridge::resume_webview(&wv);
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

    if *state.profile_panel_open.lock().unwrap() {
        sync_bounds_on_resize(app);
    }
    emit_shell_state(app);
    Ok(is_empty)
}

/// Closes all tabs except `keep_tab_id` and activates `keep_tab_id`.
pub fn close_other_tabs_sync<R: Runtime>(app: &AppHandle<R>, keep_tab_id: &str) -> Result<(), String> {
    let other_ids: Vec<String> = {
        let state = app.state::<AppState>();
        let tabs = state.tabs.lock().unwrap();
        if !tabs.iter().any(|t| t.id == keep_tab_id) {
            return Err("Tab target tidak ditemukan".to_string());
        }
        tabs.iter()
            .filter(|t| t.id != keep_tab_id)
            .map(|t| t.id.clone())
            .collect()
    };

    for id in other_ids {
        let _ = close_tab_internal(app, &id);
    }

    switch_tab_sync(app, keep_tab_id)
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
        assert!(compute_download_slide_step_rect(1600, 1025, 1.25, 0).is_none());

        let (pos, sz) =
            compute_download_slide_step_rect(1600, 1025, 1.25, 425).expect("open rect");
        assert_eq!(sz.width, 425);
        assert_eq!(pos.x, 1600 - 425);
        assert_eq!(pos.y, 98);
        assert_eq!(sz.height, 1025 - 98);
    }

    #[test]
    fn compute_idle_sleep_candidates_skips_active_sleeping_recent_and_downloading_tabs() {
        let now = 2_000_u64;
        let mk_tab = |id: &str, sleeping: bool, last_active_at: u64| TabEntry {
            id: id.into(),
            profile_id: "p1".into(),
            profile_key: "Admin".into(),
            profile_color: "#b91c1c".into(),
            is_ephemeral: false,
            is_sleeping: sleeping,
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
            mk_tab("active", false, 0),
            mk_tab("sleeping", true, 0),
            mk_tab("recent", false, now - 100),
            mk_tab("downloading", false, 0),
            mk_tab("eligible", false, now - IDLE_SLEEP_SECONDS - 1),
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
            IDLE_SLEEP_SECONDS,
        );
        assert_eq!(candidates, vec!["eligible".to_string()]);
    }
}
