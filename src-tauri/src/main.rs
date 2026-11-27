#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod downloads;
mod profile;
mod profile_transfer;
mod runtime_check;
mod state;
#[cfg(test)]
mod test_server;
mod updater;
mod webview;
mod workspace;

use tauri::webview::WebviewBuilder;
use tauri::window::{Color, WindowBuilder};
use tauri::{Emitter, Manager, WebviewUrl, WindowEvent};

use commands::{
    adjust_active_tab_zoom, check_for_updates, clear_download_history, clear_profile_data,
    close_other_tabs, close_tab, create_tab, create_tab_with_new_profile, delete_profile_dir,
    delete_workspace, duplicate_tab, export_profile_zip, find_in_active_tab, get_app_info,
    get_profile_summaries, get_shell_state, go_back_active_tab, go_forward_active_tab,
    go_home_active_tab, import_profile_zip, install_available_update, launch_workspace,
    navigate_active_tab, open_active_devtools, recover_tab, refresh_task_monitor,
    reload_active_tab, reorder_tab, save_current_workspace, set_default_profile,
    set_download_panel_open, set_find_bar_open, set_monitor_panel_open, set_preferences,
    set_profile_panel_open, set_shell_overlay_open, show_app_menu, show_download_in_folder,
    show_new_tab_menu, show_tab_context_menu, show_tab_overflow_menu, sleep_inactive_tabs,
    sleep_tab, stop_active_tab, switch_tab, toggle_download_panel, toggle_monitor_panel,
    toggle_profile_panel, toggle_tab_pin, update_profile,
};
use state::{extract_cli_url, AppState, ThemeMode};

fn main() {
    let webview2_version = match runtime_check::ensure_webview2_runtime() {
        Ok(ver) => ver,
        Err(msg) => {
            runtime_check::show_startup_error_dialog(&msg);
            std::process::exit(1);
        }
    };

    tauri::Builder::default()
        // Register single-instance FIRST so second launches never lock active WebView2 profile folders.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(window) = app.get_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            if let Some(url) = extract_cli_url(&argv) {
                let app_handle = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let (prof, ephem) = {
                        let state = app_handle.state::<AppState>();
                        let def = state.preferences.lock().unwrap().default_profile.clone();
                        webview::default_startup_profile(
                            &state.profiles_dir,
                            &state.profiles_store_path,
                            &state.profile_names_path,
                            def.as_deref(),
                        )
                    };
                    let _ = webview::spawn_tab(app_handle, prof, ephem, Some(url));
                });
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let local_data = app.path().app_local_data_dir()?;
            let config_dir = app.path().app_config_dir()?;
            let profiles_dir = local_data.join("profiles");
            let ephemeral_dir = local_data.join("ephemeral");
            let downloads_dir = app
                .path()
                .download_dir()
                .unwrap_or_else(|_| local_data.join("downloads"));
            let prefs_path = config_dir.join("preferences.json");
            let profiles_store_path = config_dir.join("profiles.json");
            let profile_names_path = config_dir.join("profile_names.json");
            std::fs::create_dir_all(&profiles_dir)?;
            std::fs::create_dir_all(&ephemeral_dir)?;
            webview::cleanup_stale_ephemeral_dirs(&ephemeral_dir);

            let app_state = AppState::new(
                webview2_version.clone(),
                profiles_dir,
                ephemeral_dir,
                downloads_dir,
                prefs_path,
                profiles_store_path,
                profile_names_path,
            );
            let window_bg = match app_state.preferences.lock().unwrap().theme {
                ThemeMode::Dark => Color(32, 33, 36, 255),
                ThemeMode::Light | ThemeMode::System => Color(241, 243, 244, 255),
            };
            app.manage(app_state);
            app.manage(updater::UpdaterState::default());

            let init_width = 1280.0;
            let init_height = 820.0;

            let mut win_builder = WindowBuilder::new(app, "main")
                .title("RoleTab")
                .inner_size(init_width, init_height)
                .min_inner_size(480.0, 320.0)
                .background_color(window_bg)
                .visible(false);
            if let Some(icon) = app.default_window_icon() {
                win_builder = win_builder.icon(icon.clone())?;
            }
            let window = win_builder.build()?;

            let app_resize = app.handle().clone();
            window.on_window_event(move |event| match event {
                WindowEvent::Resized(size) => {
                    webview::sync_bounds_with_size(&app_resize, size.width, size.height);
                }
                WindowEvent::ScaleFactorChanged { .. }
                | WindowEvent::Moved(_)
                | WindowEvent::Focused(true) => {
                    webview::sync_bounds_on_resize(&app_resize);
                }
                WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => {
                    app_resize.exit(0);
                }
                _ => {}
            });

            let scale = window.scale_factor().unwrap_or(1.0);
            let inner = window.inner_size()?;
            let (shell_rect, _) =
                webview::compute_physical_layout_rects(inner.width, inner.height, scale);

            let shell_builder = WebviewBuilder::new("ui-shell", WebviewUrl::App("index.html".into()));
            window.add_child(shell_builder, shell_rect.position, shell_rect.size)?;
            webview::sync_bounds_on_resize(app.handle());
            let _ = window.show();

            let app_menu = app.handle().clone();
            app.on_menu_event(move |_app_handle, event| {
                let id = event.id().as_ref().to_string();
                if let Some(tab_id) = id.strip_prefix("overflow-switch:") {
                    let _ = webview::switch_tab_sync(&app_menu, tab_id);
                    return;
                }
                if let Some(tab_id) = id.strip_prefix("ctx-pin:") {
                    let _ = webview::toggle_tab_pin_sync(&app_menu, tab_id);
                    return;
                }
                if let Some(tab_id) = id.strip_prefix("ctx-sleep:") {
                    let _ = webview::sleep_tab_sync(&app_menu, tab_id);
                    return;
                }
                if let Some(tab_id) = id.strip_prefix("ctx-close-others:") {
                    let _ = webview::close_other_tabs_sync(&app_menu, tab_id);
                    return;
                }
                if let Some(tab_id) = id.strip_prefix("ctx-close:") {
                    let handle = app_menu.clone();
                    let target = tab_id.to_string();
                    tauri::async_runtime::spawn(async move {
                        let _ = close_tab(handle, target).await;
                    });
                    return;
                }

                if id == "new-tab:create-profile" {
                    let _ = app_menu.emit_to("ui-shell", "open-new-profile-modal", ());
                    return;
                }
                if id == "menu:profile-manager" {
                    let handle = app_menu.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        let _ = webview::set_profile_panel_open_sync(&handle, true);
                    });
                    return;
                }
                if id == "menu:download-panel" {
                    let handle = app_menu.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        let _ = webview::set_download_panel_open_sync(&handle, true);
                    });
                    return;
                }
                if id == "menu:task-monitor" {
                    let handle = app_menu.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        let _ = webview::set_monitor_panel_open_sync(&handle, true);
                    });
                    return;
                }
                if id == "menu:devtools" {
                    let handle = app_menu.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = open_active_devtools(handle).await;
                    });
                    return;
                }
                if id == "menu:about" {
                    let _ = app_menu.emit_to("ui-shell", "open-about-modal", ());
                    return;
                }

                let spawn_handle = app_menu.clone();
                if let Some(profile_key) = id.strip_prefix("new-tab:profile:") {
                    let target_profile = profile_key.to_string();
                    tauri::async_runtime::spawn_blocking(move || {
                        let _ = webview::spawn_tab(spawn_handle, target_profile, false, None);
                    });
                    return;
                }
                if id == "new-tab:ephemeral" {
                    tauri::async_runtime::spawn_blocking(move || {
                        let _ = webview::spawn_tab(spawn_handle, "ephemeral".into(), true, None);
                    });
                    return;
                }

                if let Some(tab_id) = id.strip_prefix("ctx-dup-same:") {
                    let target = tab_id.to_string();
                    tauri::async_runtime::spawn_blocking(move || {
                        let _ = webview::duplicate_tab_sync(spawn_handle, &target, None, None);
                    });
                    return;
                }
                if let Some(rest) = id.strip_prefix("ctx-dup-profile:") {
                    if let Some((tab_id, target_profile)) = rest.split_once(':') {
                        let target = tab_id.to_string();
                        let profile = target_profile.to_string();
                        tauri::async_runtime::spawn_blocking(move || {
                            let _ = webview::duplicate_tab_sync(
                                spawn_handle,
                                &target,
                                Some(profile),
                                Some(false),
                            );
                        });
                    }
                    return;
                }
                if let Some(tab_id) = id.strip_prefix("ctx-dup-ephem:") {
                    let target = tab_id.to_string();
                    tauri::async_runtime::spawn_blocking(move || {
                        let _ = webview::duplicate_tab_sync(
                            spawn_handle,
                            &target,
                            Some("ephemeral".into()),
                            Some(true),
                        );
                    });
                    return;
                }
            });

            // Startup default: spawn 1 initial tab off the setup thread.
            let app_init = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                let (profile_key, is_ephemeral) = {
                    let state = app_init.state::<AppState>();
                    let def = state.preferences.lock().unwrap().default_profile.clone();
                    webview::default_startup_profile(
                        &state.profiles_dir,
                        &state.profiles_store_path,
                        &state.profile_names_path,
                        def.as_deref(),
                    )
                };
                let _ = webview::spawn_tab(app_init, profile_key, is_ephemeral, None);
            });

            // Lightweight background scheduler: refreshes memory metrics and checks smart idle sleep every 15s.
            let app_sleep = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(15));
                webview::run_idle_sleep_check(&app_sleep);
            });

            // Non-blocking startup updater check (4s delay so cold start stays instant).
            let app_upd = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(4));
                let result = tauri::async_runtime::block_on(updater::check_for_updates_async(&app_upd));
                if result.available {
                    let _ = app_upd.emit_to("ui-shell", "update-available", result);
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_shell_state,
            get_app_info,
            check_for_updates,
            install_available_update,
            get_profile_summaries,
            set_default_profile,
            update_profile,
            clear_profile_data,
            set_profile_panel_open,
            toggle_profile_panel,
            set_download_panel_open,
            toggle_download_panel,
            set_monitor_panel_open,
            toggle_monitor_panel,
            refresh_task_monitor,
            show_download_in_folder,
            clear_download_history,
            sleep_inactive_tabs,
            toggle_tab_pin,
            recover_tab,
            adjust_active_tab_zoom,
            set_find_bar_open,
            find_in_active_tab,
            save_current_workspace,
            delete_workspace,
            launch_workspace,
            export_profile_zip,
            import_profile_zip,
            set_preferences,
            create_tab,
            create_tab_with_new_profile,
            set_shell_overlay_open,
            switch_tab,
            duplicate_tab,
            reorder_tab,
            navigate_active_tab,
            go_back_active_tab,
            go_forward_active_tab,
            reload_active_tab,
            stop_active_tab,
            go_home_active_tab,
            close_tab,
            close_other_tabs,
            sleep_tab,
            delete_profile_dir,
            show_new_tab_menu,
            show_app_menu,
            show_tab_context_menu,
            show_tab_overflow_menu,
            open_active_devtools
        ])
        .run(tauri::generate_context!())
        .expect("error while running RoleTab");
}
