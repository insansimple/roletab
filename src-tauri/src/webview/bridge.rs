use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, Webview};

/// Payload emitted to `ui-shell` when a keyboard shortcut is intercepted inside a child webview.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutEventPayload {
    pub tab_id: String,
    pub action: String,
}

/// Payload emitted to `ui-shell` when `ICoreWebView2_3::TrySuspend` completes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuspendResultPayload {
    pub tab_id: String,
    pub hr_ok: bool,
    pub is_successful: bool,
}

/// Real-time navigation state pushed from native WebView2 events.
#[derive(Debug, Clone, Default)]
pub struct NativeNavState {
    pub tab_id: String,
    pub url: Option<String>,
    pub can_go_back: Option<bool>,
    pub can_go_forward: Option<bool>,
    pub is_loading: Option<bool>,
    pub has_cert_warning: Option<bool>,
    pub favicon_url: Option<Option<String>>,
}

/// Maps `COREWEBVIEW2_WEB_ERROR_STATUS` integer values `1..=5` (TLS/certificate errors).
pub fn is_certificate_web_error(status: i32) -> bool {
    matches!(status, 1..=5)
}

/// Maps a Win32 virtual key + modifier state to a RoleTab shortcut action.
pub fn map_accelerator_key(vk: u32, ctrl: bool, shift: bool) -> Option<&'static str> {
    const VK_TAB: u32 = 0x09;
    const VK_L: u32 = 0x4C;
    const VK_N: u32 = 0x4E;
    const VK_R: u32 = 0x52;
    const VK_T: u32 = 0x54;
    const VK_W: u32 = 0x57;
    const VK_F5: u32 = 0x74;
    const VK_F12: u32 = 0x7B;

    if ctrl && shift && vk == VK_N {
        return Some("new_ephemeral_tab");
    }
    if ctrl && shift && vk == VK_TAB {
        return Some("prev_tab");
    }
    if ctrl && !shift {
        return match vk {
            VK_T => Some("new_tab"),
            VK_W => Some("close_tab"),
            VK_TAB => Some("next_tab"),
            VK_L => Some("focus_address_bar"),
            VK_R => Some("reload_tab"),
            _ => None,
        };
    }
    if !ctrl && !shift {
        return match vk {
            VK_F5 => Some("reload_tab"),
            VK_F12 => Some("toggle_devtools"),
            _ => None,
        };
    }
    None
}

#[cfg(windows)]
unsafe fn ensure_clip_siblings(
    ctrl: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
) -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, SetWindowLongW, GWL_STYLE, WS_CLIPSIBLINGS,
    };
    let mut hwnd = HWND::default();
    if ctrl.ParentWindow(&mut hwnd).is_ok() && !hwnd.0.is_null() {
        let style = GetWindowLongW(hwnd, GWL_STYLE);
        let next_style = style | (WS_CLIPSIBLINGS.0 as i32);
        if style != next_style {
            let _ = SetWindowLongW(hwnd, GWL_STYLE, next_style);
        }
        return Some(hwnd);
    }
    None
}

/// Hooks `AcceleratorKeyPressed` on `ICoreWebView2Controller` and native navigation/favicon events
/// (`SourceChanged`, `HistoryChanged`, `NavigationStarting`, `NavigationCompleted`, `FaviconChanged`).
#[cfg(windows)]
pub fn attach_native_hooks<R: Runtime>(
    webview: &Webview<R>,
    app: AppHandle<R>,
    tab_id: String,
) -> Result<(), String> {
    webview
        .with_webview(move |platform_wv| unsafe {
            use webview2_com::Microsoft::Web::WebView2::Win32::{
                ICoreWebView2_15, COREWEBVIEW2_KEY_EVENT_KIND,
                COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
                COREWEBVIEW2_WEB_ERROR_STATUS,
            };
            use webview2_com::{
                take_pwstr, AcceleratorKeyPressedEventHandler, FaviconChangedEventHandler,
                HistoryChangedEventHandler, NavigationCompletedEventHandler,
                NavigationStartingEventHandler, SourceChangedEventHandler,
            };
            use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
            use windows_core::{Interface, BOOL, PWSTR};

            let controller = platform_wv.controller();
            let _ = ensure_clip_siblings(&controller);

            let app_key = app.clone();
            let tab_key = tab_id.clone();
            let key_handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };

                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                    && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                {
                    return Ok(());
                }

                let mut vk: u32 = 0;
                args.VirtualKey(&mut vk)?;
                let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;

                if let Some(action) = map_accelerator_key(vk, ctrl, shift) {
                    let _ = args.SetHandled(true);
                    let _ = app_key.emit_to(
                        "ui-shell",
                        "shell-shortcut",
                        ShortcutEventPayload {
                            tab_id: tab_key.clone(),
                            action: action.to_string(),
                        },
                    );
                }

                Ok(())
            }));

            let mut token: i64 = 0;
            let _ = controller.add_AcceleratorKeyPressed(&key_handler, &mut token);

            let Ok(core) = controller.CoreWebView2() else {
                return;
            };

            let app_src = app.clone();
            let tab_src = tab_id.clone();
            let src_handler = SourceChangedEventHandler::create(Box::new(move |wv, _| {
                if let Some(wv) = wv {
                    let mut raw = PWSTR::null();
                    if wv.Source(&mut raw).is_ok() {
                        let url = take_pwstr(raw);
                        crate::webview::apply_native_nav_update(
                            &app_src,
                            NativeNavState {
                                tab_id: tab_src.clone(),
                                url: Some(url),
                                ..Default::default()
                            },
                        );
                    }
                }
                Ok(())
            }));
            let _ = core.add_SourceChanged(&src_handler, &mut token);

            let app_hist = app.clone();
            let tab_hist = tab_id.clone();
            let hist_handler = HistoryChangedEventHandler::create(Box::new(move |wv, _| {
                if let Some(wv) = wv {
                    let mut back = BOOL::default();
                    let mut fwd = BOOL::default();
                    let _ = wv.CanGoBack(&mut back);
                    let _ = wv.CanGoForward(&mut fwd);
                    crate::webview::apply_native_nav_update(
                        &app_hist,
                        NativeNavState {
                            tab_id: tab_hist.clone(),
                            can_go_back: Some(back.as_bool()),
                            can_go_forward: Some(fwd.as_bool()),
                            ..Default::default()
                        },
                    );
                }
                Ok(())
            }));
            let _ = core.add_HistoryChanged(&hist_handler, &mut token);

            let app_start = app.clone();
            let tab_start = tab_id.clone();
            let start_handler = NavigationStartingEventHandler::create(Box::new(move |_, _| {
                crate::webview::apply_native_nav_update(
                    &app_start,
                    NativeNavState {
                        tab_id: tab_start.clone(),
                        is_loading: Some(true),
                        has_cert_warning: Some(false),
                        ..Default::default()
                    },
                );
                Ok(())
            }));
            let _ = core.add_NavigationStarting(&start_handler, &mut token);

            let app_done = app.clone();
            let tab_done = tab_id.clone();
            let done_handler = NavigationCompletedEventHandler::create(Box::new(move |wv, args| {
                let mut back = BOOL::default();
                let mut fwd = BOOL::default();
                if let Some(wv) = wv {
                    let _ = wv.CanGoBack(&mut back);
                    let _ = wv.CanGoForward(&mut fwd);
                }
                let mut has_cert_warning = false;
                if let Some(args) = args {
                    let mut is_success = BOOL::default();
                    let mut web_status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                    let _ = args.IsSuccess(&mut is_success);
                    let _ = args.WebErrorStatus(&mut web_status);
                    if !is_success.as_bool() && is_certificate_web_error(web_status.0) {
                        has_cert_warning = true;
                    }
                }
                crate::webview::apply_native_nav_update(
                    &app_done,
                    NativeNavState {
                        tab_id: tab_done.clone(),
                        can_go_back: Some(back.as_bool()),
                        can_go_forward: Some(fwd.as_bool()),
                        is_loading: Some(false),
                        has_cert_warning: Some(has_cert_warning),
                        ..Default::default()
                    },
                );
                Ok(())
            }));
            let _ = core.add_NavigationCompleted(&done_handler, &mut token);

            if let Ok(wv15) = core.cast::<ICoreWebView2_15>() {
                let app_fav = app;
                let tab_fav = tab_id;
                let fav_handler = FaviconChangedEventHandler::create(Box::new(move |wv, _| {
                    if let Some(wv) = wv {
                        if let Ok(inner15) = wv.cast::<ICoreWebView2_15>() {
                            let mut raw = PWSTR::null();
                            if inner15.FaviconUri(&mut raw).is_ok() {
                                let uri = take_pwstr(raw);
                                let next_fav = if uri.trim().is_empty() { None } else { Some(uri) };
                                crate::webview::apply_native_nav_update(
                                    &app_fav,
                                    NativeNavState {
                                        tab_id: tab_fav.clone(),
                                        favicon_url: Some(next_fav),
                                        ..Default::default()
                                    },
                                );
                            }
                        }
                    }
                    Ok(())
                }));
                let _ = wv15.add_FaviconChanged(&fav_handler, &mut token);
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn attach_native_hooks<R: Runtime>(
    _webview: &Webview<R>,
    _app: AppHandle<R>,
    _tab_id: String,
) -> Result<(), String> {
    Ok(())
}

/// Navigates backward in the child webview's WebView2 history.
#[cfg(windows)]
pub fn go_back_webview<R: Runtime>(webview: &Webview<R>) -> Result<(), String> {
    webview
        .with_webview(|platform_wv| unsafe {
            if let Ok(core) = platform_wv.controller().CoreWebView2() {
                let _ = core.GoBack();
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn go_back_webview<R: Runtime>(_webview: &Webview<R>) -> Result<(), String> {
    Ok(())
}

/// Navigates forward in the child webview's WebView2 history.
#[cfg(windows)]
pub fn go_forward_webview<R: Runtime>(webview: &Webview<R>) -> Result<(), String> {
    webview
        .with_webview(|platform_wv| unsafe {
            if let Ok(core) = platform_wv.controller().CoreWebView2() {
                let _ = core.GoForward();
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn go_forward_webview<R: Runtime>(_webview: &Webview<R>) -> Result<(), String> {
    Ok(())
}

/// Stops an in-progress navigation in the child webview.
#[cfg(windows)]
pub fn stop_webview<R: Runtime>(webview: &Webview<R>) -> Result<(), String> {
    webview
        .with_webview(|platform_wv| unsafe {
            if let Ok(core) = platform_wv.controller().CoreWebView2() {
                let _ = core.Stop();
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn stop_webview<R: Runtime>(_webview: &Webview<R>) -> Result<(), String> {
    Ok(())
}

/// Calls `ICoreWebView2_3::TrySuspend` on a hidden child webview and emits completion to `ui-shell`.
#[cfg(windows)]
pub fn suspend_webview<R: Runtime>(
    webview: &Webview<R>,
    app: AppHandle<R>,
    tab_id: String,
) -> Result<(), String> {
    webview
        .with_webview(move |platform_wv| unsafe {
            use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
            use webview2_com::TrySuspendCompletedHandler;
            use windows_core::Interface;

            let controller = platform_wv.controller();
            let _ = ensure_clip_siblings(&controller);
            let _ = controller.SetIsVisible(false);

            let Ok(core) = controller.CoreWebView2() else {
                let _ = app.emit_to(
                    "ui-shell",
                    "shell-suspend-result",
                    SuspendResultPayload {
                        tab_id,
                        hr_ok: false,
                        is_successful: false,
                    },
                );
                return;
            };

            let Ok(wv3) = core.cast::<ICoreWebView2_3>() else {
                let _ = app.emit_to(
                    "ui-shell",
                    "shell-suspend-result",
                    SuspendResultPayload {
                        tab_id,
                        hr_ok: false,
                        is_successful: false,
                    },
                );
                return;
            };

            let app_clone = app.clone();
            let tab_clone = tab_id.clone();
            let handler = TrySuspendCompletedHandler::create(Box::new(move |hr, is_successful| {
                let suspended = hr.is_ok() && is_successful;
                crate::webview::apply_suspend_outcome(&app_clone, &tab_clone, suspended);
                let _ = app_clone.emit_to(
                    "ui-shell",
                    "shell-suspend-result",
                    SuspendResultPayload {
                        tab_id: tab_clone.clone(),
                        hr_ok: hr.is_ok(),
                        is_successful,
                    },
                );
                Ok(())
            }));

            if wv3.TrySuspend(&handler).is_err() {
                crate::webview::apply_suspend_outcome(&app, &tab_id, false);
                let _ = app.emit_to(
                    "ui-shell",
                    "shell-suspend-result",
                    SuspendResultPayload {
                        tab_id,
                        hr_ok: false,
                        is_successful: false,
                    },
                );
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn suspend_webview<R: Runtime>(
    _webview: &Webview<R>,
    _app: AppHandle<R>,
    _tab_id: String,
) -> Result<(), String> {
    Ok(())
}

/// Calls `ICoreWebView2_3::Resume` on a previously suspended child webview.
#[cfg(windows)]
pub fn resume_webview<R: Runtime>(webview: &Webview<R>) -> Result<(), String> {
    webview
        .with_webview(move |platform_wv| unsafe {
            use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
            use windows_core::Interface;

            if let Ok(core) = platform_wv.controller().CoreWebView2() {
                if let Ok(wv3) = core.cast::<ICoreWebView2_3>() {
                    let _ = wv3.Resume();
                }
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn resume_webview<R: Runtime>(_webview: &Webview<R>) -> Result<(), String> {
    Ok(())
}

/// Synchronously updates a child webview's WebView2 bounds and Win32 HWND position/size
/// without `SWP_ASYNCWINDOWPOS`, eliminating right-edge black blink during resizes/slides.
#[cfg(windows)]
pub fn set_bounds_sync_native<R: Runtime>(
    webview: &Webview<R>,
    pos: tauri::PhysicalPosition<i32>,
    size: tauri::PhysicalSize<u32>,
    bring_to_top: bool,
    ensure_visible: bool,
) -> Result<(), String> {
    webview
        .with_webview(move |platform_wv| unsafe {
            use windows::Win32::Foundation::RECT;
            use windows::Win32::UI::WindowsAndMessaging::{
                SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW,
            };

            let w = (size.width as i32).max(1);
            let h = (size.height as i32).max(1);
            let ctrl = platform_wv.controller();
            let _ = ctrl.SetBounds(RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            });

            if let Some(hwnd) = ensure_clip_siblings(&ctrl) {
                let mut flags = SWP_NOACTIVATE;
                let z_insert = if bring_to_top {
                    Some(HWND_TOP)
                } else {
                    flags |= SWP_NOZORDER;
                    None
                };
                if ensure_visible {
                    let _ = ctrl.SetIsVisible(true);
                    flags |= SWP_SHOWWINDOW;
                }
                let _ = SetWindowPos(hwnd, z_insert, pos.x, pos.y, w, h, flags);
            }
            let _ = ctrl.NotifyParentWindowPositionChanged();
        })
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn set_bounds_sync_native<R: Runtime>(
    webview: &Webview<R>,
    pos: tauri::PhysicalPosition<i32>,
    size: tauri::PhysicalSize<u32>,
    _bring_to_top: bool,
    ensure_visible: bool,
) -> Result<(), String> {
    let _ = webview.set_bounds(tauri::Rect {
        position: pos.into(),
        size: size.into(),
    });
    if ensure_visible {
        let _ = webview.show();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_certificate_web_error, map_accelerator_key};

    #[test]
    fn maps_all_fr2_shortcuts_accurately() {
        assert_eq!(map_accelerator_key(0x54, true, false), Some("new_tab"));
        assert_eq!(map_accelerator_key(0x57, true, false), Some("close_tab"));
        assert_eq!(map_accelerator_key(0x09, true, false), Some("next_tab"));
        assert_eq!(map_accelerator_key(0x09, true, true), Some("prev_tab"));
        assert_eq!(map_accelerator_key(0x4C, true, false), Some("focus_address_bar"));
        assert_eq!(map_accelerator_key(0x52, true, false), Some("reload_tab"));
        assert_eq!(map_accelerator_key(0x74, false, false), Some("reload_tab"));
        assert_eq!(map_accelerator_key(0x7B, false, false), Some("toggle_devtools"));
        assert_eq!(map_accelerator_key(0x4E, true, true), Some("new_ephemeral_tab"));
    }

    #[test]
    fn ignores_regular_typing_and_unmapped_keys() {
        assert_eq!(map_accelerator_key(0x41, false, false), None);
        assert_eq!(map_accelerator_key(0x43, true, false), None);
        assert_eq!(map_accelerator_key(0x56, true, false), None);
    }

    #[test]
    fn detects_webview2_certificate_error_codes() {
        for code in 1..=5 {
            assert!(is_certificate_web_error(code));
        }
        assert!(!is_certificate_web_error(0));
        assert!(!is_certificate_web_error(6));
    }
}
