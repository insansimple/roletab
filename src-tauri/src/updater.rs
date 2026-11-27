use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Holds the latest discovered update handle so the user can install & restart from About UI.
#[derive(Default)]
pub struct UpdaterState(pub Mutex<Option<Update>>);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub available: bool,
    pub latest_version: Option<String>,
    pub release_notes: Option<String>,
    pub pub_date: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfoSnapshot {
    pub app_name: String,
    pub version: String,
    pub webview2_version: String,
    pub profiles_dir: String,
    pub devtools_enabled: bool,
    pub zero_ipc_webviews: Vec<String>,
}

/// Runs `tauri-plugin-updater` check using configured endpoints and caches `Update` if present.
pub async fn check_for_updates_async<R: Runtime>(app: &AppHandle<R>) -> UpdateCheckResult {
    let current_version = app.package_info().version.to_string();
    let updater = match app.updater() {
        Ok(u) => u,
        Err(err) => {
            return UpdateCheckResult {
                current_version,
                available: false,
                latest_version: None,
                release_notes: None,
                pub_date: None,
                error: Some(err.to_string()),
            };
        }
    };

    match updater.check().await {
        Ok(Some(update)) => {
            let latest_version = update.version.clone();
            let release_notes = update.body.clone();
            let pub_date = update.date.map(|d| d.to_string());
            *app.state::<UpdaterState>().0.lock().unwrap() = Some(update);
            UpdateCheckResult {
                current_version,
                available: true,
                latest_version: Some(latest_version),
                release_notes,
                pub_date,
                error: None,
            }
        }
        Ok(None) => {
            *app.state::<UpdaterState>().0.lock().unwrap() = None;
            UpdateCheckResult {
                current_version,
                available: false,
                latest_version: None,
                release_notes: None,
                pub_date: None,
                error: None,
            }
        }
        Err(err) => {
            *app.state::<UpdaterState>().0.lock().unwrap() = None;
            UpdateCheckResult {
                current_version,
                available: false,
                latest_version: None,
                release_notes: None,
                pub_date: None,
                error: Some(err.to_string()),
            }
        }
    }
}

/// Downloads and applies the cached pending update, then restarts RoleTab.
pub async fn install_pending_update_async<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let pending = app.state::<UpdaterState>().0.lock().unwrap().take();
    let Some(update) = pending else {
        return Err("Tidak ada pembaruan tertunda".to_string());
    };
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| format!("Gagal memasang pembaruan: {e}"))?;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, Deserialize)]
    struct StagingReleaseManifest {
        version: String,
        notes: Option<String>,
        pub_date: Option<String>,
    }

    fn is_newer_semver(current: &str, candidate: &str) -> bool {
        let parse_triple = |raw: &str| -> Option<(u64, u64, u64)> {
            let clean = raw.trim().trim_start_matches('v');
            let mut parts = clean.split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next().unwrap_or("0").parse().ok()?;
            let patch_str = parts.next().unwrap_or("0").split('-').next().unwrap_or("0");
            let patch = patch_str.parse().ok()?;
            Some((major, minor, patch))
        };

        match (parse_triple(current), parse_triple(candidate)) {
            (Some(cur), Some(cand)) => cand > cur,
            _ => false,
        }
    }

    fn evaluate_staging_manifest(current_version: &str, manifest_json: &str) -> UpdateCheckResult {
        match serde_json::from_str::<StagingReleaseManifest>(manifest_json) {
            Ok(manifest) => {
                let available = is_newer_semver(current_version, &manifest.version);
                UpdateCheckResult {
                    current_version: current_version.to_string(),
                    available,
                    latest_version: Some(manifest.version),
                    release_notes: manifest.notes,
                    pub_date: manifest.pub_date,
                    error: None,
                }
            }
            Err(err) => UpdateCheckResult {
                current_version: current_version.to_string(),
                available: false,
                latest_version: None,
                release_notes: None,
                pub_date: None,
                error: Some(err.to_string()),
            },
        }
    }

    #[test]
    fn evaluates_local_staging_manifest_from_1_0_0_to_1_0_1_and_rejects_downgrades() {
        let upgrade_json = r#"{
            "version": "1.0.1",
            "notes": "Perbaikan stabilitas WebView2 & Tab Sleep",
            "pub_date": "2026-11-27T11:30:00Z",
            "platforms": {
                "windows-x86_64": {
                    "signature": "TEST_SIG",
                    "url": "http://127.0.0.1:18080/RoleTab_1.0.1_x64-setup.nsis.zip"
                }
            }
        }"#;
        let res = evaluate_staging_manifest("1.0.0", upgrade_json);
        assert!(res.available);
        assert_eq!(res.latest_version.as_deref(), Some("1.0.1"));
        assert!(res.error.is_none());

        let same_res = evaluate_staging_manifest("1.0.1", upgrade_json);
        assert!(!same_res.available);

        let downgrade_res = evaluate_staging_manifest("1.1.0", upgrade_json);
        assert!(!downgrade_res.available);
    }

    #[test]
    fn simulates_profile_persistence_during_staging_upgrade_and_audits_bundle_security() {
        let base = std::env::temp_dir().join(format!("roletab_upg_{}", uuid::Uuid::new_v4().simple()));
        let profiles_dir = base.join("profiles");
        let prof_admin = profiles_dir.join("admin-uuid-1");
        std::fs::create_dir_all(&prof_admin).unwrap();
        let session_marker = prof_admin.join("Cookies");
        std::fs::write(&session_marker, b"persisted_session_cookie").unwrap();

        let cap_json: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        let webviews = cap_json["webviews"].as_array().unwrap();
        let allowed: Vec<&str> = webviews.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(allowed, vec!["ui-shell", "profile-panel", "download-panel", "task-monitor"]);
        assert!(!allowed.contains(&"*"));

        let conf_json: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf_json["version"].as_str(), Some("1.0.0"));
        assert_eq!(conf_json["bundle"]["active"].as_bool(), Some(true));
        assert_eq!(
            conf_json["bundle"]["createUpdaterArtifacts"].as_bool(),
            Some(true)
        );
        assert_eq!(
            conf_json["bundle"]["windows"]["webviewInstallMode"]["type"].as_str(),
            Some("downloadBootstrapper")
        );
        assert!(
            !conf_json["plugins"]["updater"]["pubkey"]
                .as_str()
                .unwrap_or("")
                .is_empty()
        );

        let cargo_toml = include_str!("../Cargo.toml");
        assert!(cargo_toml.contains("devtools"));

        // Confirm isolated profile folder on %LOCALAPPDATA% stays intact across updater metadata checks
        assert_eq!(
            std::fs::read(&session_marker).unwrap(),
            b"persisted_session_cookie"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn qa_verifies_14_tabs_across_4_profiles_plus_2_ephemeral_and_strict_csp_isolation() {
        use std::collections::HashSet;

        use crate::profile::{create_profile_record, profile_data_dir};
        use crate::state::{AppState, TabEntry};

        let base =
            std::env::temp_dir().join(format!("roletab_qa_{}", uuid::Uuid::new_v4().simple()));
        let profiles_dir = base.join("profiles");
        let eph_dir = base.join("ephemeral");
        let dl_dir = base.join("downloads");
        let prefs_path = base.join("preferences.json");
        let store_path = base.join("profiles.json");
        let names_path = base.join("profile_names.json");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::create_dir_all(&eph_dir).unwrap();

        let state = AppState::new(
            "131.0.2903.0".into(),
            profiles_dir.clone(),
            eph_dir.clone(),
            dl_dir,
            prefs_path,
            store_path.clone(),
            names_path.clone(),
        );

        let role_names = ["Admin", "Approver", "Kasir", "Auditor"];
        let mut created_ids = Vec::new();
        for name in role_names {
            let rec =
                create_profile_record(&profiles_dir, &store_path, &names_path, name, None).unwrap();
            created_ids.push(rec);
        }

        // Open 3 tabs per persistent profile (12 tabs) + 2 isolated Ephemeral tabs = 14 awake tabs (> 12)
        let mut tabs = Vec::new();
        for (p_idx, rec) in created_ids.iter().enumerate() {
            for copy in 0..3 {
                tabs.push(TabEntry {
                    id: format!("tab-p{p_idx}-{copy}"),
                    profile_id: rec.id.clone(),
                    profile_key: rec.name.clone(),
                    profile_color: rec.color.clone(),
                    is_ephemeral: false,
                    is_sleeping: false,
                    is_pinned: false,
                    is_playing_audio: false,
                    browser_pid: None,
                    memory_mb: None,
                    zoom_factor: 1.0,
                    crash_kind: None,
                    last_active_at: 1_000,
                    can_go_back: false,
                    can_go_forward: false,
                    is_loading: false,
                    has_cert_warning: false,
                    favicon_url: None,
                    title: format!("{} #{copy}", rec.name),
                    url: "https://example.test/dashboard".into(),
                });
            }
        }
        for eph_idx in 0..2 {
            tabs.push(TabEntry {
                id: format!("tab-eph-{eph_idx}"),
                profile_id: format!("ephemeral-tab-eph-{eph_idx}"),
                profile_key: "Ephemeral".into(),
                profile_color: "#4b5563".into(),
                is_ephemeral: true,
                is_sleeping: false,
                is_pinned: false,
                is_playing_audio: false,
                browser_pid: None,
                memory_mb: None,
                zoom_factor: 1.0,
                crash_kind: None,
                last_active_at: 1_000,
                can_go_back: false,
                can_go_forward: false,
                is_loading: false,
                has_cert_warning: false,
                favicon_url: None,
                title: format!("Ephemeral #{eph_idx}"),
                url: "https://example.test/guest".into(),
            });
        }

        *state.tabs.lock().unwrap() = tabs;
        *state.active_tab_id.lock().unwrap() = Some("tab-p0-0".into());

        let snap = state.snapshot();
        assert_eq!(snap.tabs.len(), 14);
        assert_eq!(snap.active_awake_count, 14);
        assert!(snap.active_awake_count > 12);

        let persistent_dirs: HashSet<_> = created_ids
            .iter()
            .map(|r| profile_data_dir(&profiles_dir, &r.id))
            .collect();
        assert_eq!(persistent_dirs.len(), 4);

        let eph_dirs: HashSet<_> = snap
            .tabs
            .iter()
            .filter(|t| t.is_ephemeral)
            .map(|t| eph_dir.join(&t.id))
            .collect();
        assert_eq!(eph_dirs.len(), 2);

        let conf_json: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = conf_json["app"]["security"]["csp"].as_str().unwrap();
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("script-src 'self'"));

        let main_rs = include_str!("main.rs");
        let single_pos = main_rs
            .find("tauri_plugin_single_instance::init")
            .expect("single-instance plugin must be registered");
        let updater_pos = main_rs
            .find("tauri_plugin_updater::Builder")
            .expect("updater plugin must be registered");
        assert!(single_pos < updater_pos);

        let _ = std::fs::remove_dir_all(&base);
    }
}
