use std::path::PathBuf;

use tauri::webview::WebviewBuilder;
use tauri::{AppHandle, PhysicalPosition, PhysicalSize, Runtime, Webview, WebviewWindowBuilder};

/// Cross-platform abstraction separating Windows (`webview2-com`) lifecycle/control
/// from macOS (`WKWebView` / `data_store_identifier`) and generic Tauri runtimes.
pub trait PlatformWebviewController: Send + Sync {
    #[allow(dead_code)]
    fn backend_name(&self) -> &'static str;

    fn attach_lifecycle_bridge<R: Runtime>(
        &self,
        webview: &Webview<R>,
        app: AppHandle<R>,
        tab_id: String,
    ) -> Result<(), String>;

    fn suspend_tab<R: Runtime>(
        &self,
        webview: &Webview<R>,
        app: AppHandle<R>,
        tab_id: String,
    ) -> Result<(), String>;

    fn resume_tab<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String>;

    fn go_back<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String>;

    fn go_forward<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String>;

    fn stop<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String>;

    fn set_zoom<R: Runtime>(&self, webview: &Webview<R>, factor: f64) -> Result<(), String>;

    fn find_in_page<R: Runtime>(
        &self,
        webview: &Webview<R>,
        query: &str,
        forward: bool,
    ) -> Result<usize, String>;

    #[allow(dead_code)]
    fn read_process_memory_mb(&self, pid: u32) -> Option<u64>;

    fn set_bounds_sync<R: Runtime>(
        &self,
        webview: &Webview<R>,
        pos: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
        bring_to_top: bool,
        ensure_visible: bool,
    ) -> Result<(), String>;
}

#[cfg(target_os = "windows")]
pub struct WindowsWebviewController;

#[cfg(target_os = "windows")]
impl PlatformWebviewController for WindowsWebviewController {
    fn backend_name(&self) -> &'static str {
        "windows-webview2"
    }

    fn attach_lifecycle_bridge<R: Runtime>(
        &self,
        webview: &Webview<R>,
        app: AppHandle<R>,
        tab_id: String,
    ) -> Result<(), String> {
        super::bridge::attach_native_hooks(webview, app, tab_id)
    }

    fn suspend_tab<R: Runtime>(
        &self,
        webview: &Webview<R>,
        app: AppHandle<R>,
        tab_id: String,
    ) -> Result<(), String> {
        super::bridge::suspend_webview(webview, app, tab_id)
    }

    fn resume_tab<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String> {
        super::bridge::resume_webview(webview)
    }

    fn go_back<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String> {
        super::bridge::go_back_webview(webview)
    }

    fn go_forward<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String> {
        super::bridge::go_forward_webview(webview)
    }

    fn stop<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String> {
        super::bridge::stop_webview(webview)
    }

    fn set_zoom<R: Runtime>(&self, webview: &Webview<R>, factor: f64) -> Result<(), String> {
        super::bridge::set_webview_zoom(webview, factor)
    }

    fn find_in_page<R: Runtime>(
        &self,
        webview: &Webview<R>,
        query: &str,
        forward: bool,
    ) -> Result<usize, String> {
        super::bridge::find_in_webview(webview, query, forward)
    }

    fn read_process_memory_mb(&self, pid: u32) -> Option<u64> {
        super::bridge::read_process_working_set_mb(pid)
    }

    fn set_bounds_sync<R: Runtime>(
        &self,
        webview: &Webview<R>,
        pos: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
        bring_to_top: bool,
        ensure_visible: bool,
    ) -> Result<(), String> {
        super::bridge::set_bounds_sync_native(webview, pos, size, bring_to_top, ensure_visible)
    }
}

#[cfg(target_os = "macos")]
pub struct MacosWebviewController;

#[cfg(target_os = "macos")]
impl PlatformWebviewController for MacosWebviewController {
    fn backend_name(&self) -> &'static str {
        "macos-wkwebview"
    }

    fn attach_lifecycle_bridge<R: Runtime>(
        &self,
        _webview: &Webview<R>,
        _app: AppHandle<R>,
        _tab_id: String,
    ) -> Result<(), String> {
        Ok(())
    }

    fn suspend_tab<R: Runtime>(
        &self,
        webview: &Webview<R>,
        app: AppHandle<R>,
        tab_id: String,
    ) -> Result<(), String> {
        let _ = webview.hide();
        super::apply_suspend_outcome(&app, &tab_id, true);
        Ok(())
    }

    fn resume_tab<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String> {
        let _ = webview.show();
        Ok(())
    }

    fn go_back<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String> {
        webview
            .eval("window.history.back();")
            .map_err(|e| e.to_string())
    }

    fn go_forward<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String> {
        webview
            .eval("window.history.forward();")
            .map_err(|e| e.to_string())
    }

    fn stop<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String> {
        webview.eval("window.stop();").map_err(|e| e.to_string())
    }

    fn set_zoom<R: Runtime>(&self, webview: &Webview<R>, factor: f64) -> Result<(), String> {
        webview
            .set_zoom(factor.clamp(0.25, 3.0))
            .map_err(|e| e.to_string())
    }

    fn find_in_page<R: Runtime>(
        &self,
        webview: &Webview<R>,
        query: &str,
        forward: bool,
    ) -> Result<usize, String> {
        let q_json = serde_json::to_string(query).map_err(|e| e.to_string())?;
        let backwards = if forward { "false" } else { "true" };
        let script = format!(
            "(() => {{ const q = {q_json}; if (!q) {{ window.getSelection()?.removeAllRanges(); return; }} window.find(q, false, {backwards}, true, false, false, false); }})()"
        );
        webview.eval(&script).map_err(|e| e.to_string())?;
        Ok(0)
    }

    fn read_process_memory_mb(&self, _pid: u32) -> Option<u64> {
        None
    }

    fn set_bounds_sync<R: Runtime>(
        &self,
        webview: &Webview<R>,
        pos: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
        _bring_to_top: bool,
        ensure_visible: bool,
    ) -> Result<(), String> {
        webview
            .set_bounds(tauri::Rect {
                position: pos.into(),
                size: size.into(),
            })
            .map_err(|e| e.to_string())?;
        if ensure_visible {
            let _ = webview.show();
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub struct GenericWebviewController;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
impl PlatformWebviewController for GenericWebviewController {
    fn backend_name(&self) -> &'static str {
        "generic"
    }

    fn attach_lifecycle_bridge<R: Runtime>(
        &self,
        _webview: &Webview<R>,
        _app: AppHandle<R>,
        _tab_id: String,
    ) -> Result<(), String> {
        Ok(())
    }

    fn suspend_tab<R: Runtime>(
        &self,
        webview: &Webview<R>,
        app: AppHandle<R>,
        tab_id: String,
    ) -> Result<(), String> {
        let _ = webview.hide();
        super::apply_suspend_outcome(&app, &tab_id, true);
        Ok(())
    }

    fn resume_tab<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String> {
        let _ = webview.show();
        Ok(())
    }

    fn go_back<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String> {
        webview
            .eval("window.history.back();")
            .map_err(|e| e.to_string())
    }

    fn go_forward<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String> {
        webview
            .eval("window.history.forward();")
            .map_err(|e| e.to_string())
    }

    fn stop<R: Runtime>(&self, webview: &Webview<R>) -> Result<(), String> {
        webview.eval("window.stop();").map_err(|e| e.to_string())
    }

    fn set_zoom<R: Runtime>(&self, webview: &Webview<R>, factor: f64) -> Result<(), String> {
        webview
            .set_zoom(factor.clamp(0.25, 3.0))
            .map_err(|e| e.to_string())
    }

    fn find_in_page<R: Runtime>(
        &self,
        webview: &Webview<R>,
        query: &str,
        forward: bool,
    ) -> Result<usize, String> {
        let q_json = serde_json::to_string(query).map_err(|e| e.to_string())?;
        let backwards = if forward { "false" } else { "true" };
        let script = format!(
            "(() => {{ const q = {q_json}; if (!q) {{ window.getSelection()?.removeAllRanges(); return; }} window.find(q, false, {backwards}, true, false, false, false); }})()"
        );
        webview.eval(&script).map_err(|e| e.to_string())?;
        Ok(0)
    }

    fn read_process_memory_mb(&self, _pid: u32) -> Option<u64> {
        None
    }

    fn set_bounds_sync<R: Runtime>(
        &self,
        webview: &Webview<R>,
        pos: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
        _bring_to_top: bool,
        ensure_visible: bool,
    ) -> Result<(), String> {
        webview
            .set_bounds(tauri::Rect {
                position: pos.into(),
                size: size.into(),
            })
            .map_err(|e| e.to_string())?;
        if ensure_visible {
            let _ = webview.show();
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub const ACTIVE_CONTROLLER: WindowsWebviewController = WindowsWebviewController;

#[cfg(target_os = "macos")]
pub const ACTIVE_CONTROLLER: MacosWebviewController = MacosWebviewController;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const ACTIVE_CONTROLLER: GenericWebviewController = GenericWebviewController;

/// Computes a deterministic RFC-4122 UUID `[u8; 16]` for a persistent `profile_id`,
/// powering macOS >= 14 `WKWebsiteDataStore(forIdentifier:)` via `WebviewBuilder::data_store_identifier`.
#[allow(dead_code)]
pub fn profile_data_store_identifier(profile_id: &str) -> [u8; 16] {
    fn fnv1a_64(seed: u64, input: &[u8]) -> u64 {
        let mut h = seed;
        for &b in input {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    let hi = fnv1a_64(0xcbf29ce484222325, profile_id.as_bytes());
    let lo = fnv1a_64(0x9e3779b97f4a7c15, profile_id.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(&hi.to_be_bytes());
    bytes[8..].copy_from_slice(&lo.to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}

/// Applies platform-appropriate session storage isolation to a child `WebviewBuilder`:
/// - Windows: `.data_directory(data_dir)` + optional `.incognito(true)`
/// - macOS (>= 14): `.incognito(true)` for Ephemeral or `.data_store_identifier(uuid)` for Persistent Role
pub fn configure_child_isolation<R: Runtime>(
    mut builder: WebviewBuilder<R>,
    is_ephemeral: bool,
    data_dir: PathBuf,
    profile_id: &str,
) -> WebviewBuilder<R> {
    #[cfg(target_os = "macos")]
    {
        let _ = data_dir;
        if is_ephemeral {
            builder = builder.incognito(true);
        } else {
            builder = builder.data_store_identifier(profile_data_store_identifier(profile_id));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = profile_id;
        builder = builder.data_directory(data_dir);
        if is_ephemeral {
            builder = builder.incognito(true);
        }
    }
    builder
}

/// Applies matching session isolation to temporary OAuth/SSO popup windows.
pub fn configure_popup_isolation<'a, R: Runtime, M: tauri::Manager<R>>(
    mut builder: WebviewWindowBuilder<'a, R, M>,
    is_ephemeral: bool,
    data_dir: PathBuf,
    profile_id: &str,
) -> WebviewWindowBuilder<'a, R, M> {
    #[cfg(target_os = "macos")]
    {
        let _ = data_dir;
        if is_ephemeral {
            builder = builder.incognito(true);
        } else {
            builder = builder.data_store_identifier(profile_data_store_identifier(profile_id));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = profile_id;
        builder = builder.data_directory(data_dir);
        if is_ephemeral {
            builder = builder.incognito(true);
        }
    }
    builder
}

/// Maps macOS `Cmd+*` keyboard shortcuts (`Cmd+T`, `Cmd+W`, `Cmd+L`, `Cmd+R`, `Cmd+N`,
/// `Cmd+Shift+N`, `Cmd+Option+I`, `Cmd+F`, zoom, tab switch) cleanly onto RoleTab actions.
#[allow(dead_code)]
pub fn map_macos_shortcut(key: &str, cmd: bool, shift: bool, alt: bool) -> Option<&'static str> {
    let lower = key.trim().to_ascii_lowercase();
    if cmd && alt && lower == "i" {
        return Some("toggle_devtools");
    }
    if cmd && shift && lower == "n" {
        return Some("new_ephemeral_tab");
    }
    if cmd && shift && lower == "tab" {
        return Some("prev_tab");
    }
    if cmd && !shift && !alt {
        return match lower.as_str() {
            "t" => Some("new_tab"),
            "w" => Some("close_tab"),
            "n" => Some("new_window"),
            "l" => Some("focus_address_bar"),
            "r" => Some("reload_tab"),
            "f" => Some("find_in_page"),
            "=" | "+" => Some("zoom_in"),
            "-" => Some("zoom_out"),
            "0" => Some("zoom_reset"),
            "tab" => Some("next_tab"),
            _ => None,
        };
    }
    None
}

/// Installs standard macOS application menu bar (`RoleTab`, `File`, `Edit`, `View`, `Window`)
/// binding native `Cmd+*` accelerators to RoleTab shell actions.
#[cfg(target_os = "macos")]
pub fn install_macos_app_menu<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};

    let app_sub = SubmenuBuilder::new(app, "RoleTab")
        .item(&MenuItemBuilder::with_id("menu:about", "About RoleTab & Updates").build(app).map_err(|e| e.to_string())?)
        .separator()
        .item(&PredefinedMenuItem::hide(app, None).map_err(|e| e.to_string())?)
        .item(&PredefinedMenuItem::hide_others(app, None).map_err(|e| e.to_string())?)
        .separator()
        .item(&PredefinedMenuItem::quit(app, None).map_err(|e| e.to_string())?)
        .build()
        .map_err(|e| e.to_string())?;

    let file_sub = SubmenuBuilder::new(app, "File")
        .item(&MenuItemBuilder::with_id("mac:new-tab", "New Tab").accelerator("CmdOrCtrl+T").build(app).map_err(|e| e.to_string())?)
        .item(&MenuItemBuilder::with_id("new-tab:ephemeral", "New Ephemeral Tab").accelerator("CmdOrCtrl+Shift+N").build(app).map_err(|e| e.to_string())?)
        .item(&MenuItemBuilder::with_id("menu:new-window", "New Window").accelerator("CmdOrCtrl+N").build(app).map_err(|e| e.to_string())?)
        .separator()
        .item(&MenuItemBuilder::with_id("mac:close-tab", "Close Tab").accelerator("CmdOrCtrl+W").build(app).map_err(|e| e.to_string())?)
        .build()
        .map_err(|e| e.to_string())?;

    let edit_sub = SubmenuBuilder::new(app, "Edit")
        .item(&PredefinedMenuItem::undo(app, None).map_err(|e| e.to_string())?)
        .item(&PredefinedMenuItem::redo(app, None).map_err(|e| e.to_string())?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, None).map_err(|e| e.to_string())?)
        .item(&PredefinedMenuItem::copy(app, None).map_err(|e| e.to_string())?)
        .item(&PredefinedMenuItem::paste(app, None).map_err(|e| e.to_string())?)
        .item(&PredefinedMenuItem::select_all(app, None).map_err(|e| e.to_string())?)
        .build()
        .map_err(|e| e.to_string())?;

    let view_sub = SubmenuBuilder::new(app, "View")
        .item(&MenuItemBuilder::with_id("mac:reload", "Reload Tab").accelerator("CmdOrCtrl+R").build(app).map_err(|e| e.to_string())?)
        .item(&MenuItemBuilder::with_id("mac:focus-url", "Focus Address Bar").accelerator("CmdOrCtrl+L").build(app).map_err(|e| e.to_string())?)
        .separator()
        .item(&MenuItemBuilder::with_id("menu:profile-manager", "Profile Manager").build(app).map_err(|e| e.to_string())?)
        .item(&MenuItemBuilder::with_id("menu:download-panel", "Downloads").build(app).map_err(|e| e.to_string())?)
        .item(&MenuItemBuilder::with_id("menu:task-monitor", "Task Monitor").build(app).map_err(|e| e.to_string())?)
        .item(&MenuItemBuilder::with_id("menu:devtools", "Developer Tools").accelerator("Cmd+Alt+I").build(app).map_err(|e| e.to_string())?)
        .build()
        .map_err(|e| e.to_string())?;

    let window_sub = SubmenuBuilder::new(app, "Window")
        .item(&PredefinedMenuItem::minimize(app, None).map_err(|e| e.to_string())?)
        .item(&PredefinedMenuItem::maximize(app, None).map_err(|e| e.to_string())?)
        .build()
        .map_err(|e| e.to_string())?;

    let menu = MenuBuilder::new(app)
        .items(&[&app_sub, &file_sub, &edit_sub, &view_sub, &window_sub])
        .build()
        .map_err(|e| e.to_string())?;
    app.set_menu(menu).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_deterministic_uuid_data_store_identifiers_per_profile() {
        let a1 = profile_data_store_identifier("prof-admin-1");
        let a2 = profile_data_store_identifier("prof-admin-1");
        let b1 = profile_data_store_identifier("prof-user-2");
        assert_eq!(a1, a2);
        assert_ne!(a1, b1);
        assert_eq!(a1[6] & 0xf0, 0x40);
        assert_eq!(a1[8] & 0xc0, 0x80);
        #[cfg(target_os = "windows")]
        assert_eq!(ACTIVE_CONTROLLER.backend_name(), "windows-webview2");
        #[cfg(target_os = "macos")]
        assert_eq!(ACTIVE_CONTROLLER.backend_name(), "macos-wkwebview");
    }

    #[test]
    fn maps_macos_cmd_shortcuts_to_roletab_actions() {
        assert_eq!(map_macos_shortcut("t", true, false, false), Some("new_tab"));
        assert_eq!(map_macos_shortcut("w", true, false, false), Some("close_tab"));
        assert_eq!(map_macos_shortcut("l", true, false, false), Some("focus_address_bar"));
        assert_eq!(map_macos_shortcut("r", true, false, false), Some("reload_tab"));
        assert_eq!(map_macos_shortcut("n", true, false, false), Some("new_window"));
        assert_eq!(map_macos_shortcut("n", true, true, false), Some("new_ephemeral_tab"));
        assert_eq!(map_macos_shortcut("i", true, false, true), Some("toggle_devtools"));
        assert_eq!(map_macos_shortcut("f", true, false, false), Some("find_in_page"));
        assert_eq!(map_macos_shortcut("tab", true, true, false), Some("prev_tab"));
        assert_eq!(map_macos_shortcut("q", true, false, false), None);
    }
}
