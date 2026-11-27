use std::path::Path;
use std::time::{Duration, Instant};

use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, LogicalPosition, Manager, State};

use crate::downloads::reveal_in_explorer;
use crate::profile::{
    build_profile_summaries, clear_closed_profile_disk_data, create_profile_record,
    delete_dir_with_retry, profile_data_dir, remove_profile_record, sanitize_profile_key,
    set_default_profile_record, sync_profiles_store, update_profile_record, ProfileRecord,
    ProfileSummary, ProxyMode,
};
use crate::profile_transfer::{import_profile_zip_to_dir, pick_open_zip_path};
use crate::state::{
    save_preferences, AppPreferences, AppState, ClearProfileDataResult, DeleteProfileResult,
    DownloadStatus, Language, ShellSnapshot, TabEntry,
};
use crate::updater::{
    check_for_updates_async, install_pending_update_async, AppInfoSnapshot, UpdateCheckResult,
};
use crate::webview::{
    adjust_active_tab_zoom_in_window, close_other_tabs_sync, close_tab_internal,
    default_startup_profile, detach_tab_to_new_window_sync, drop_tab_at_screen_point_sync,
    duplicate_tab_sync, emit_shell_state, export_profile_archive_sync,
    find_in_active_tab_in_window, launch_workspace_in_window, move_tab_to_window_sync,
    normalize_address_input, platform::{self, PlatformWebviewController}, recover_tab_sync,
    refresh_memory_metrics, reorder_window_tabs_vec, set_download_panel_open_in_window,
    set_find_bar_open_in_window, set_monitor_panel_open_in_window,
    set_profile_panel_open_in_window, shell_label_for_window, sleep_inactive_tabs_sync,
    sleep_tab_sync, spawn_browser_window, spawn_tab_in_window, switch_tab_sync,
    sync_bounds_on_resize, sync_bounds_on_resize_for_window, toggle_tab_pin_sync,
    window_label_from_webview_label, DEFAULT_NEW_TAB_URL, UI_SHELL_HEIGHT,
};
use crate::workspace::{
    create_workspace_preset, delete_workspace_preset, specs_from_open_tabs, WorkspacePreset,
};

#[tauri::command]
pub fn get_shell_state(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    state: State<'_, AppState>,
) -> ShellSnapshot {
    let win_label = window_label_from_webview_label(webview.label());
    state.ensure_window(&win_label);
    sync_bounds_on_resize_for_window(&app, &win_label);
    state.snapshot_for_window(&win_label)
}

#[tauri::command]
pub fn set_preferences(
    app: AppHandle<tauri::Wry>,
    preferences: AppPreferences,
) -> Result<AppPreferences, String> {
    let state = app.state::<AppState>();
    save_preferences(&state.prefs_path, &preferences)?;
    *state.preferences.lock().unwrap() = preferences.clone();
    sync_bounds_on_resize(&app);
    emit_shell_state(&app);
    Ok(preferences)
}

#[tauri::command]
pub async fn create_window(
    app: AppHandle<tauri::Wry>,
    profile_key: Option<String>,
    is_ephemeral: Option<bool>,
    url: Option<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        spawn_browser_window(app, profile_key, is_ephemeral, url, None)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn detach_tab_to_new_window(
    app: AppHandle<tauri::Wry>,
    tab_id: String,
    screen_x: Option<i32>,
    screen_y: Option<i32>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        detach_tab_to_new_window_sync(app, &tab_id, screen_x, screen_y)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn move_tab_to_window(
    app: AppHandle<tauri::Wry>,
    tab_id: String,
    target_window_label: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        move_tab_to_window_sync(app, &tab_id, &target_window_label)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn drop_tab_at_screen_point(
    app: AppHandle<tauri::Wry>,
    tab_id: String,
    screen_x: i32,
    screen_y: i32,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        drop_tab_at_screen_point_sync(app, &tab_id, screen_x, screen_y)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn create_tab(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    profile_key: String,
    is_ephemeral: bool,
    url: Option<String>,
) -> Result<TabEntry, String> {
    let win_label = window_label_from_webview_label(webview.label());
    tauri::async_runtime::spawn_blocking(move || {
        spawn_tab_in_window(app, &win_label, profile_key, is_ephemeral, url)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn switch_tab(app: AppHandle<tauri::Wry>, tab_id: String) -> Result<(), String> {
    switch_tab_sync(&app, &tab_id)
}

#[tauri::command]
pub async fn duplicate_tab(
    app: AppHandle<tauri::Wry>,
    source_tab_id: String,
    target_profile: Option<String>,
    target_ephemeral: Option<bool>,
) -> Result<TabEntry, String> {
    tauri::async_runtime::spawn_blocking(move || {
        duplicate_tab_sync(app, &source_tab_id, target_profile, target_ephemeral)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn reorder_tab(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    from_index: usize,
    to_index: usize,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    let changed = {
        let state = app.state::<AppState>();
        let mut tabs = state.tabs.lock().unwrap();
        reorder_window_tabs_vec(&mut tabs, &win_label, from_index, to_index)
    };
    if changed {
        emit_shell_state(&app);
    }
    Ok(())
}

#[tauri::command]
pub async fn navigate_active_tab(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    url: String,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    let active_id = app
        .state::<AppState>()
        .active_tab_for(&win_label)
        .ok_or_else(|| "Tidak ada tab aktif".to_string())?;
    let parsed = normalize_address_input(&url)?;
    if let Some(wv) = app.get_webview(&active_id) {
        wv.navigate(parsed).map_err(|e| e.to_string())?;
        let _ = wv.set_focus();
    }
    Ok(())
}

#[tauri::command]
pub async fn go_back_active_tab(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    if let Some(id) = app.state::<AppState>().active_tab_for(&win_label) {
        if let Some(wv) = app.get_webview(&id) {
            platform::ACTIVE_CONTROLLER.go_back(&wv)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn go_forward_active_tab(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    if let Some(id) = app.state::<AppState>().active_tab_for(&win_label) {
        if let Some(wv) = app.get_webview(&id) {
            platform::ACTIVE_CONTROLLER.go_forward(&wv)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn reload_active_tab(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    if let Some(id) = app.state::<AppState>().active_tab_for(&win_label) {
        if let Some(wv) = app.get_webview(&id) {
            wv.reload().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn stop_active_tab(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    if let Some(id) = app.state::<AppState>().active_tab_for(&win_label) {
        if let Some(wv) = app.get_webview(&id) {
            platform::ACTIVE_CONTROLLER.stop(&wv)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn go_home_active_tab(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    if let Some(id) = app.state::<AppState>().active_tab_for(&win_label) {
        if let Some(wv) = app.get_webview(&id) {
            let parsed: url::Url = DEFAULT_NEW_TAB_URL.parse().map_err(|e: url::ParseError| e.to_string())?;
            wv.navigate(parsed).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn create_tab_with_new_profile(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    profile_name: String,
    color: Option<String>,
) -> Result<TabEntry, String> {
    let win_label = window_label_from_webview_label(webview.label());
    let record = {
        let state = app.state::<AppState>();
        let rec = create_profile_record(
            &state.profiles_dir,
            &state.profiles_store_path,
            &state.profile_names_path,
            &profile_name,
            color.as_deref(),
        )?;
        state.update_window_state(&win_label, |w| {
            w.shell_overlay_open = false;
        });
        rec
    };

    let app_spawn = app.clone();
    let win_spawn = win_label.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        spawn_tab_in_window(app_spawn, &win_spawn, record.id, false, None)
    })
    .await
    .map_err(|e| e.to_string())?;

    sync_bounds_on_resize_for_window(&app, &win_label);
    res
}

#[tauri::command]
pub fn update_profile(
    app: AppHandle<tauri::Wry>,
    profile_id: String,
    name: String,
    color: Option<String>,
    proxy_mode: Option<ProxyMode>,
    proxy_address: Option<String>,
) -> Result<ProfileRecord, String> {
    let updated = {
        let state = app.state::<AppState>();
        let rec = update_profile_record(
            &state.profiles_dir,
            &state.profiles_store_path,
            &state.profile_names_path,
            &profile_id,
            &name,
            color.as_deref(),
            proxy_mode,
            proxy_address,
        )?;
        let mut tabs = state.tabs.lock().unwrap();
        for tab in tabs.iter_mut() {
            if !tab.is_ephemeral && tab.profile_id == rec.id {
                tab.profile_key = rec.name.clone();
                tab.profile_color = rec.color.clone();
            }
        }
        rec
    };

    emit_shell_state(&app);
    Ok(updated)
}

#[tauri::command]
pub async fn clear_profile_data(
    app: AppHandle<tauri::Wry>,
    profile_id: String,
    reload_tabs: bool,
) -> Result<ClearProfileDataResult, String> {
    let safe_id = sanitize_profile_key(&profile_id);
    let (target_dir, tab_ids) = {
        let state = app.state::<AppState>();
        let tabs = state.tabs.lock().unwrap();
        let ids: Vec<String> = tabs
            .iter()
            .filter(|t| !t.is_ephemeral && t.profile_id == safe_id)
            .map(|t| t.id.clone())
            .collect();
        (profile_data_dir(&state.profiles_dir, &safe_id), ids)
    };

    let mut cleared_tabs = 0_usize;
    let mut reloaded_tabs = 0_usize;

    if tab_ids.is_empty() {
        tauri::async_runtime::spawn_blocking(move || clear_closed_profile_disk_data(&target_dir))
            .await
            .map_err(|e| e.to_string())??;
    } else {
        for tid in tab_ids {
            if let Some(wv) = app.get_webview(&tid) {
                if cleared_tabs == 0 {
                    wv.clear_all_browsing_data().map_err(|e| e.to_string())?;
                }
                cleared_tabs += 1;
                if reload_tabs && wv.reload().is_ok() {
                    reloaded_tabs += 1;
                }
            }
        }
    }

    emit_shell_state(&app);
    Ok(ClearProfileDataResult {
        profile_id: safe_id,
        cleared_tabs,
        reloaded_tabs,
        message: format!("Data profil dibersihkan ({cleared_tabs} tab)"),
    })
}

#[tauri::command]
pub fn set_shell_overlay_open(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    open: bool,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    let active_id = {
        let state = app.state::<AppState>();
        state.update_window_state(&win_label, |w| {
            w.shell_overlay_open = open;
        });
        state.active_tab_for(&win_label)
    };
    sync_bounds_on_resize_for_window(&app, &win_label);
    if open {
        if let Some(shell) = app.get_webview(&shell_label_for_window(&win_label)) {
            let _ = shell.set_focus();
        }
    } else if let Some(tab_id) = active_id {
        if let Some(wv) = app.get_webview(&tab_id) {
            let _ = wv.set_focus();
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn set_profile_panel_open(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    open: bool,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    tauri::async_runtime::spawn_blocking(move || {
        set_profile_panel_open_in_window(&app, &win_label, open)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn toggle_profile_panel(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    let next = !app
        .state::<AppState>()
        .window_state(&win_label)
        .profile_panel_open;
    tauri::async_runtime::spawn_blocking(move || {
        set_profile_panel_open_in_window(&app, &win_label, next)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_download_panel_open(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    open: bool,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    tauri::async_runtime::spawn_blocking(move || {
        set_download_panel_open_in_window(&app, &win_label, open)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn toggle_download_panel(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    let next = !app
        .state::<AppState>()
        .window_state(&win_label)
        .download_panel_open;
    tauri::async_runtime::spawn_blocking(move || {
        set_download_panel_open_in_window(&app, &win_label, next)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn show_download_in_folder(
    app: AppHandle<tauri::Wry>,
    download_id: String,
) -> Result<(), String> {
    let path_str = {
        let state = app.state::<AppState>();
        let downloads = state.downloads.lock().unwrap();
        downloads
            .iter()
            .find(|d| d.id == download_id)
            .map(|d| d.path.clone())
            .ok_or_else(|| "Item unduhan tidak ditemukan".to_string())?
    };
    reveal_in_explorer(Path::new(&path_str))
}

#[tauri::command]
pub fn clear_download_history(app: AppHandle<tauri::Wry>) -> Result<(), String> {
    let state = app.state::<AppState>();
    {
        let mut downloads = state.downloads.lock().unwrap();
        downloads.retain(|d| d.status == DownloadStatus::InProgress);
    }
    emit_shell_state(&app);
    Ok(())
}

#[tauri::command]
pub async fn sleep_inactive_tabs(app: AppHandle<tauri::Wry>) -> Result<usize, String> {
    sleep_inactive_tabs_sync(&app)
}

#[tauri::command]
pub async fn set_monitor_panel_open(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    open: bool,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    tauri::async_runtime::spawn_blocking(move || {
        set_monitor_panel_open_in_window(&app, &win_label, open)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn toggle_monitor_panel(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    let next = !app
        .state::<AppState>()
        .window_state(&win_label)
        .monitor_panel_open;
    tauri::async_runtime::spawn_blocking(move || {
        set_monitor_panel_open_in_window(&app, &win_label, next)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn refresh_task_monitor(app: AppHandle<tauri::Wry>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || refresh_memory_metrics(&app))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn toggle_tab_pin(app: AppHandle<tauri::Wry>, tab_id: String) -> Result<bool, String> {
    toggle_tab_pin_sync(&app, &tab_id)
}

#[tauri::command]
pub async fn recover_tab(app: AppHandle<tauri::Wry>, tab_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || recover_tab_sync(app, &tab_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn adjust_active_tab_zoom(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    delta: Option<f64>,
) -> Result<f64, String> {
    let win_label = window_label_from_webview_label(webview.label());
    adjust_active_tab_zoom_in_window(&app, &win_label, delta)
}

#[tauri::command]
pub async fn set_find_bar_open(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    open: bool,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    set_find_bar_open_in_window(&app, &win_label, open)
}

#[tauri::command]
pub async fn find_in_active_tab(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    query: String,
    forward: bool,
) -> Result<usize, String> {
    let win_label = window_label_from_webview_label(webview.label());
    find_in_active_tab_in_window(&app, &win_label, &query, forward)
}

#[tauri::command]
pub fn save_current_workspace(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    name: String,
    accent_color: Option<String>,
) -> Result<WorkspacePreset, String> {
    let win_label = window_label_from_webview_label(webview.label());
    let state = app.state::<AppState>();
    let specs = {
        let tabs = state.tabs.lock().unwrap();
        let win_tabs: Vec<TabEntry> = tabs
            .iter()
            .filter(|t| t.window_label == win_label)
            .cloned()
            .collect();
        specs_from_open_tabs(&win_tabs)
    };
    let created = create_workspace_preset(&state.workspaces_path, &name, specs, accent_color)?;
    emit_shell_state(&app);
    Ok(created)
}

#[tauri::command]
pub fn delete_workspace(app: AppHandle<tauri::Wry>, workspace_id: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    delete_workspace_preset(&state.workspaces_path, &workspace_id)?;
    emit_shell_state(&app);
    Ok(())
}

#[tauri::command]
pub async fn launch_workspace(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    workspace_id: String,
) -> Result<usize, String> {
    let win_label = window_label_from_webview_label(webview.label());
    tauri::async_runtime::spawn_blocking(move || {
        launch_workspace_in_window(app, &win_label, &workspace_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn export_profile_zip(
    app: AppHandle<tauri::Wry>,
    profile_id: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || export_profile_archive_sync(&app, &profile_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn import_profile_zip(
    app: AppHandle<tauri::Wry>,
    source_path: Option<String>,
) -> Result<Option<ProfileRecord>, String> {
    let (profiles_dir, store_path, names_path) = {
        let state = app.state::<AppState>();
        (
            state.profiles_dir.clone(),
            state.profiles_store_path.clone(),
            state.profile_names_path.clone(),
        )
    };
    let imported = tauri::async_runtime::spawn_blocking(move || {
        let zip_path = match source_path {
            Some(p) => std::path::PathBuf::from(p),
            None => match pick_open_zip_path() {
                Some(p) => p,
                None => return Ok(None),
            },
        };
        import_profile_zip_to_dir(&profiles_dir, &store_path, &names_path, &zip_path).map(Some)
    })
    .await
    .map_err(|e| e.to_string())??;

    if imported.is_some() {
        emit_shell_state(&app);
    }
    Ok(imported)
}

#[tauri::command]
pub fn get_app_info(app: AppHandle<tauri::Wry>) -> AppInfoSnapshot {
    let state = app.state::<AppState>();
    let cap_json: serde_json::Value =
        serde_json::from_str(include_str!("../../capabilities/default.json")).unwrap_or_default();
    let privileged = cap_json["webviews"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    AppInfoSnapshot {
        app_name: app.package_info().name.clone(),
        version: app.package_info().version.to_string(),
        webview2_version: state.webview2_version.clone(),
        profiles_dir: state.profiles_dir.to_string_lossy().to_string(),
        devtools_enabled: true,
        zero_ipc_webviews: privileged,
    }
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle<tauri::Wry>) -> Result<UpdateCheckResult, String> {
    Ok(check_for_updates_async(&app).await)
}

#[tauri::command]
pub async fn install_available_update(app: AppHandle<tauri::Wry>) -> Result<(), String> {
    install_pending_update_async(&app).await
}

#[tauri::command]
pub async fn get_profile_summaries(app: AppHandle<tauri::Wry>) -> Result<Vec<ProfileSummary>, String> {
    let (profiles_dir, profiles_store_path, profile_names_path, tabs, default_profile) = {
        let state = app.state::<AppState>();
        let tabs = state.tabs.lock().unwrap().clone();
        let default_profile = state.preferences.lock().unwrap().default_profile.clone();
        (
            state.profiles_dir.clone(),
            state.profiles_store_path.clone(),
            state.profile_names_path.clone(),
            tabs,
            default_profile,
        )
    };

    tauri::async_runtime::spawn_blocking(move || {
        build_profile_summaries(
            &profiles_dir,
            &profiles_store_path,
            &profile_names_path,
            &tabs,
            default_profile.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_default_profile(
    app: AppHandle<tauri::Wry>,
    profile_key: Option<String>,
) -> Result<AppPreferences, String> {
    let state = app.state::<AppState>();
    let target_id = profile_key
        .map(|k| sanitize_profile_key(&k))
        .filter(|k| !k.is_empty());

    set_default_profile_record(
        &state.profiles_dir,
        &state.profiles_store_path,
        &state.profile_names_path,
        target_id.as_deref(),
    )?;

    let mut prefs = state.preferences.lock().unwrap().clone();
    prefs.default_profile = target_id;
    save_preferences(&state.prefs_path, &prefs)?;
    *state.preferences.lock().unwrap() = prefs.clone();
    emit_shell_state(&app);
    Ok(prefs)
}

#[tauri::command]
pub async fn close_tab(app: AppHandle<tauri::Wry>, tab_id: String) -> Result<(), String> {
    let (win_label, fallback_profile) = {
        let state = app.state::<AppState>();
        let tabs = state.tabs.lock().unwrap();
        let target_win = tabs
            .iter()
            .find(|t| t.id == tab_id)
            .map(|t| t.window_label.clone())
            .unwrap_or_else(|| "main".to_string());
        let win_tab_count = tabs.iter().filter(|t| t.window_label == target_win).count();
        let fb = if win_tab_count <= 1 && tabs.iter().any(|t| t.id == tab_id) {
            let def = state.preferences.lock().unwrap().default_profile.clone();
            Some(default_startup_profile(
                &state.profiles_dir,
                &state.profiles_store_path,
                &state.profile_names_path,
                def.as_deref(),
            ))
        } else {
            None
        };
        (target_win, fb)
    };

    // Spawn the replacement tab BEFORE closing the last tab in that window so viewport never flashes.
    if let Some((profile_key, is_ephemeral)) = fallback_profile {
        let app_spawn = app.clone();
        let win_spawn = win_label.clone();
        tauri::async_runtime::spawn_blocking(move || {
            spawn_tab_in_window(app_spawn, &win_spawn, profile_key, is_ephemeral, None)
        })
        .await
        .map_err(|e| e.to_string())??;
    }

    let is_empty = close_tab_internal(&app, &tab_id)?;
    if is_empty {
        let (profile_key, is_ephemeral) = {
            let state = app.state::<AppState>();
            let def = state.preferences.lock().unwrap().default_profile.clone();
            default_startup_profile(
                &state.profiles_dir,
                &state.profiles_store_path,
                &state.profile_names_path,
                def.as_deref(),
            )
        };
        let win_spawn = win_label.clone();
        tauri::async_runtime::spawn_blocking(move || {
            spawn_tab_in_window(app, &win_spawn, profile_key, is_ephemeral, None)
        })
        .await
        .map_err(|e| e.to_string())??;
    }
    Ok(())
}

#[tauri::command]
pub async fn close_other_tabs(app: AppHandle<tauri::Wry>, keep_tab_id: String) -> Result<(), String> {
    close_other_tabs_sync(&app, &keep_tab_id)
}

#[tauri::command]
pub async fn sleep_tab(app: AppHandle<tauri::Wry>, tab_id: String) -> Result<(), String> {
    sleep_tab_sync(&app, &tab_id)
}

#[tauri::command]
pub async fn delete_profile_dir(
    app: AppHandle<tauri::Wry>,
    profile_key: String,
) -> Result<DeleteProfileResult, String> {
    let start = Instant::now();
    let safe_key = sanitize_profile_key(&profile_key);
    let (target_dir, profiles_store_path, prefs_path, tabs_to_close) = {
        let state = app.state::<AppState>();
        let tabs = state.tabs.lock().unwrap();
        let to_close: Vec<String> = tabs
            .iter()
            .filter(|t| {
                !t.is_ephemeral
                    && (t.profile_id == safe_key || sanitize_profile_key(&t.profile_key) == safe_key)
            })
            .map(|t| t.id.clone())
            .collect();
        (
            profile_data_dir(&state.profiles_dir, &safe_key),
            state.profiles_store_path.clone(),
            state.prefs_path.clone(),
            to_close,
        )
    };

    let closed_count = tabs_to_close.len();
    let mut became_empty = false;
    for tab_id in tabs_to_close {
        became_empty = close_tab_internal(&app, &tab_id)?;
    }

    let delete_outcome = tauri::async_runtime::spawn_blocking(move || {
        delete_dir_with_retry(&target_dir, 15, Duration::from_millis(120))
    })
    .await
    .map_err(|e| e.to_string())?;

    if delete_outcome.is_ok() {
        let _ = remove_profile_record(&profiles_store_path, &safe_key);
        let state = app.state::<AppState>();
        let mut prefs = state.preferences.lock().unwrap().clone();
        if prefs
            .default_profile
            .as_deref()
            .map(sanitize_profile_key)
            .as_deref()
            == Some(safe_key.as_str())
        {
            prefs.default_profile = None;
            let _ = save_preferences(&prefs_path, &prefs);
            *state.preferences.lock().unwrap() = prefs;
        }
    }

    if became_empty {
        let app_spawn = app.clone();
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
        let win_labels = app.state::<AppState>().window_labels();
        let empty_wins: Vec<String> = {
            let state = app.state::<AppState>();
            let tabs = state.tabs.lock().unwrap();
            win_labels
                .into_iter()
                .filter(|w| !tabs.iter().any(|t| t.window_label == *w))
                .collect()
        };
        for ew in empty_wins {
            let h = app_spawn.clone();
            let fk = fallback_key.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                spawn_tab_in_window(h, &ew, fk, is_ephemeral, None)
            })
            .await;
        }
    }

    emit_shell_state(&app);
    let elapsed_ms = start.elapsed().as_millis();
    Ok(match delete_outcome {
        Ok(()) => DeleteProfileResult {
            profile_key: safe_key,
            closed_tabs: closed_count,
            deleted: true,
            elapsed_ms,
            message: format!("Profil dihapus ({closed_count} tab ditutup)"),
        },
        Err(err) => DeleteProfileResult {
            profile_key: safe_key,
            closed_tabs: closed_count,
            deleted: false,
            elapsed_ms,
            message: format!("Gagal hapus folder: {err}"),
        },
    })
}

#[tauri::command]
pub async fn show_new_tab_menu(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    let window = app
        .get_window(&win_label)
        .ok_or_else(|| format!("Window {win_label} not found"))?;

    let (profiles, is_en) = {
        let state = app.state::<AppState>();
        let prefs = state.preferences.lock().unwrap().clone();
        let profs = sync_profiles_store(
            &state.profiles_dir,
            &state.profiles_store_path,
            &state.profile_names_path,
            prefs.default_profile.as_deref(),
        );
        (profs, prefs.language == Language::En)
    };

    let mut builder = MenuBuilder::new(&app);
    for prof in &profiles {
        let item = MenuItemBuilder::with_id(
            format!("new-tab:profile:{win_label}:{}", prof.id),
            &prof.name,
        )
        .build(&app)
        .map_err(|e| e.to_string())?;
        builder = builder.item(&item);
    }

    let create_new = MenuItemBuilder::with_id(
        format!("new-tab:create-profile:{win_label}"),
        if is_en { "New Profile" } else { "Profil Baru" },
    )
    .build(&app)
    .map_err(|e| e.to_string())?;

    let sep = PredefinedMenuItem::separator(&app).map_err(|e| e.to_string())?;
    let ephem_item = MenuItemBuilder::with_id(format!("new-tab:ephemeral:{win_label}"), "Ephemeral")
        .build(&app)
        .map_err(|e| e.to_string())?;

    let menu = builder
        .item(&create_new)
        .item(&sep)
        .item(&ephem_item)
        .build()
        .map_err(|e| e.to_string())?;

    window
        .popup_menu_at(&menu, LogicalPosition::new(x.max(8.0), y.max(36.0)))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn show_app_menu(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    let window = app
        .get_window(&win_label)
        .ok_or_else(|| format!("Window {win_label} not found"))?;
    let is_en = app.state::<AppState>().preferences.lock().unwrap().language == Language::En;

    let new_win_item = MenuItemBuilder::with_id(
        "menu:new-window",
        if is_en { "New Window (Ctrl+N)" } else { "Window Baru (Ctrl+N)" },
    )
    .build(&app)
    .map_err(|e| e.to_string())?;
    let profile_mgr_item = MenuItemBuilder::with_id(
        format!("menu:profile-manager:{win_label}"),
        if is_en { "Profile Manager" } else { "Kelola Profil" },
    )
    .build(&app)
    .map_err(|e| e.to_string())?;
    let download_mgr_item = MenuItemBuilder::with_id(
        format!("menu:download-panel:{win_label}"),
        if is_en { "Downloads" } else { "Unduhan" },
    )
    .build(&app)
    .map_err(|e| e.to_string())?;
    let monitor_item = MenuItemBuilder::with_id(
        format!("menu:task-monitor:{win_label}"),
        "Task Monitor",
    )
    .build(&app)
    .map_err(|e| e.to_string())?;
    let devtools_item = MenuItemBuilder::with_id(
        format!("menu:devtools:{win_label}"),
        "DevTools (F12)",
    )
    .build(&app)
    .map_err(|e| e.to_string())?;
    let about_item = MenuItemBuilder::with_id(
        format!("menu:about:{win_label}"),
        if is_en {
            "About & Updates"
        } else {
            "Tentang & Update"
        },
    )
    .build(&app)
    .map_err(|e| e.to_string())?;
    let sep = PredefinedMenuItem::separator(&app).map_err(|e| e.to_string())?;
    let ephem_item = MenuItemBuilder::with_id(
        format!("new-tab:ephemeral:{win_label}"),
        if is_en {
            "New Ephemeral Tab (Ctrl+Shift+N)"
        } else {
            "Tab Ephemeral (Ctrl+Shift+N)"
        },
    )
    .build(&app)
    .map_err(|e| e.to_string())?;

    let menu = MenuBuilder::new(&app)
        .items(&[
            &new_win_item,
            &profile_mgr_item,
            &download_mgr_item,
            &monitor_item,
            &devtools_item,
            &about_item,
            &sep,
            &ephem_item,
        ])
        .build()
        .map_err(|e| e.to_string())?;

    let menu_x = (x - 190.0).max(16.0);
    window
        .popup_menu_at(&menu, LogicalPosition::new(menu_x, y.max(UI_SHELL_HEIGHT)))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn show_tab_context_menu(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
    tab_id: String,
    x: f64,
    y: f64,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    let window = app
        .get_window(&win_label)
        .ok_or_else(|| format!("Window {win_label} not found"))?;

    let (is_active, is_sleeping, is_pinned, win_tab_count, other_windows, profiles, is_en) = {
        let state = app.state::<AppState>();
        let active_id = state.active_tab_for(&win_label);
        let tabs = state.tabs.lock().unwrap();
        let tab = tabs
            .iter()
            .find(|t| t.id == tab_id)
            .ok_or_else(|| "Tab tidak ditemukan".to_string())?;
        let win_count = tabs
            .iter()
            .filter(|t| t.window_label == tab.window_label)
            .count();
        let other_wins: Vec<(String, String)> = state
            .window_labels()
            .into_iter()
            .enumerate()
            .filter(|(_, w)| w != &tab.window_label)
            .map(|(i, w)| (w, format!("Window {}", i + 1)))
            .collect();
        let prefs = state.preferences.lock().unwrap().clone();
        let profs = sync_profiles_store(
            &state.profiles_dir,
            &state.profiles_store_path,
            &state.profile_names_path,
            prefs.default_profile.as_deref(),
        );
        (
            active_id.as_deref() == Some(tab_id.as_str()),
            tab.is_sleeping,
            tab.is_pinned,
            win_count,
            other_wins,
            profs,
            prefs.language == Language::En,
        )
    };

    let dup_same = MenuItemBuilder::with_id(
        format!("ctx-dup-same:{tab_id}"),
        if is_en { "Duplicate Tab" } else { "Duplikat Tab" },
    )
    .build(&app)
    .map_err(|e| e.to_string())?;

    let mut dup_sub = SubmenuBuilder::new(
        &app,
        if is_en { "Duplicate to Profile" } else { "Duplikat ke Profil" },
    );
    for prof in &profiles {
        let item =
            MenuItemBuilder::with_id(format!("ctx-dup-profile:{tab_id}:{}", prof.id), &prof.name)
                .build(&app)
                .map_err(|e| e.to_string())?;
        dup_sub = dup_sub.item(&item);
    }
    let dup_ephem = MenuItemBuilder::with_id(format!("ctx-dup-ephem:{tab_id}"), "Ephemeral")
        .build(&app)
        .map_err(|e| e.to_string())?;
    let dup_submenu = dup_sub
        .item(&dup_ephem)
        .build()
        .map_err(|e| e.to_string())?;

    let detach_item = MenuItemBuilder::with_id(
        format!("ctx-detach:{tab_id}"),
        if is_en { "Move to New Window" } else { "Pisah ke Window Baru" },
    )
    .enabled(win_tab_count > 1)
    .build(&app)
    .map_err(|e| e.to_string())?;

    let move_win_submenu = if !other_windows.is_empty() {
        let mut move_sub = SubmenuBuilder::new(
            &app,
            if is_en { "Move to Window" } else { "Pindah ke Window" },
        );
        for (target_lbl, target_title) in &other_windows {
            let item = MenuItemBuilder::with_id(
                format!("ctx-move-win:{tab_id}:{target_lbl}"),
                target_title,
            )
            .build(&app)
            .map_err(|e| e.to_string())?;
            move_sub = move_sub.item(&item);
        }
        Some(move_sub.build().map_err(|e| e.to_string())?)
    } else {
        None
    };

    let pin_label = if is_pinned {
        if is_en { "Unpin Tab" } else { "Lepas Pin" }
    } else if is_en {
        "Pin / Keep Awake"
    } else {
        "Pin / Tetap Aktif"
    };
    let pin_item = MenuItemBuilder::with_id(format!("ctx-pin:{tab_id}"), pin_label)
        .build(&app)
        .map_err(|e| e.to_string())?;

    let sleep_item = MenuItemBuilder::with_id(format!("ctx-sleep:{tab_id}"), "Sleep Tab")
        .enabled(!is_active && !is_sleeping && !is_pinned)
        .build(&app)
        .map_err(|e| e.to_string())?;

    let sep = PredefinedMenuItem::separator(&app).map_err(|e| e.to_string())?;

    let close_item = MenuItemBuilder::with_id(
        format!("ctx-close:{tab_id}"),
        if is_en { "Close Tab" } else { "Tutup Tab" },
    )
    .build(&app)
    .map_err(|e| e.to_string())?;
    let close_others = MenuItemBuilder::with_id(
        format!("ctx-close-others:{tab_id}"),
        if is_en { "Close Other Tabs" } else { "Tutup Tab Lainnya" },
    )
    .enabled(win_tab_count > 1)
    .build(&app)
    .map_err(|e| e.to_string())?;

    let mut menu_builder = MenuBuilder::new(&app)
        .item(&dup_same)
        .item(&dup_submenu)
        .item(&detach_item);
    if let Some(ref move_sub) = move_win_submenu {
        menu_builder = menu_builder.item(move_sub);
    }
    let menu = menu_builder
        .item(&pin_item)
        .item(&sleep_item)
        .item(&sep)
        .item(&close_item)
        .item(&close_others)
        .build()
        .map_err(|e| e.to_string())?;

    window
        .popup_menu_at(&menu, LogicalPosition::new(x.max(0.0), y.max(0.0)))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn show_tab_overflow_menu(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    let window = app
        .get_window(&win_label)
        .ok_or_else(|| format!("Window {win_label} not found"))?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let logical_w = window
        .inner_size()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(scale)
        .width;

    let tabs: Vec<TabEntry> = app
        .state::<AppState>()
        .tabs
        .lock()
        .unwrap()
        .iter()
        .filter(|t| t.window_label == win_label)
        .cloned()
        .collect();
    let mut builder = MenuBuilder::new(&app);
    for tab in tabs {
        let role = if tab.is_ephemeral {
            "Ephemeral"
        } else {
            &tab.profile_key
        };
        let label = format!(
            "[{role}] {}{}",
            tab.title,
            if tab.is_sleeping { " (Sleep)" } else { "" }
        );
        let item = MenuItemBuilder::with_id(format!("overflow-switch:{}", tab.id), label)
            .build(&app)
            .map_err(|e| e.to_string())?;
        builder = builder.item(&item);
    }

    let menu = builder.build().map_err(|e| e.to_string())?;
    window
        .popup_menu_at(&menu, LogicalPosition::new((logical_w - 220.0).max(20.0), 38.0))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn open_active_devtools(
    app: AppHandle<tauri::Wry>,
    webview: tauri::Webview<tauri::Wry>,
) -> Result<(), String> {
    let win_label = window_label_from_webview_label(webview.label());
    if let Some(id) = app.state::<AppState>().active_tab_for(&win_label) {
        if let Some(wv) = app.get_webview(&id) {
            wv.open_devtools();
        }
    }
    Ok(())
}
