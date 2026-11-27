use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Supported UI Shell themes (`system` follows Windows preference).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    System,
    Dark,
    Light,
}

impl Default for ThemeMode {
    fn default() -> Self {
        Self::System
    }
}

/// Supported UI Shell languages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Id,
    En,
}

impl Default for Language {
    fn default() -> Self {
        Self::Id
    }
}

fn default_true() -> bool {
    true
}

/// Persistent UI Shell preferences stored locally.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppPreferences {
    pub theme: ThemeMode,
    pub language: Language,
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default = "default_true")]
    pub group_downloads_by_profile: bool,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            theme: ThemeMode::default(),
            language: Language::default(),
            default_profile: None,
            group_downloads_by_profile: true,
        }
    }
}

/// Runtime metadata for a single browser tab backed by a child WebView.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabEntry {
    pub id: String,
    pub profile_id: String,
    pub profile_key: String,
    pub profile_color: String,
    pub is_ephemeral: bool,
    pub is_sleeping: bool,
    pub last_active_at: u64,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub is_loading: bool,
    pub has_cert_warning: bool,
    pub favicon_url: Option<String>,
    pub title: String,
    pub url: String,
}

/// Lifecycle state for an intercepted download.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DownloadStatus {
    InProgress,
    Completed,
    Failed,
}

/// Metadata for a single download item tracked in `AppState`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadEntry {
    pub id: String,
    pub tab_id: String,
    pub profile_name: String,
    pub profile_color: String,
    pub url: String,
    pub file_name: String,
    pub path: String,
    pub status: DownloadStatus,
    pub started_at: u64,
    pub finished_at: Option<u64>,
}

/// Snapshot of application state synchronized to `ui-shell`, `profile-panel`, and `download-panel`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellSnapshot {
    pub webview2_version: String,
    pub profiles_dir: String,
    pub preferences: AppPreferences,
    pub profiles: Vec<crate::profile::ProfileRecord>,
    pub color_presets: Vec<String>,
    pub tabs: Vec<TabEntry>,
    pub active_tab_id: Option<String>,
    pub active_awake_count: usize,
    pub downloads: Vec<DownloadEntry>,
    pub profile_panel_open: bool,
    pub download_panel_open: bool,
}

/// Result returned after deleting a profile folder.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProfileResult {
    pub profile_key: String,
    pub closed_tabs: usize,
    pub deleted: bool,
    pub elapsed_ms: u128,
    pub message: String,
}

/// Result returned after clearing browsing data for a profile.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearProfileDataResult {
    pub profile_id: String,
    pub cleared_tabs: usize,
    pub reloaded_tabs: usize,
    pub message: String,
}

/// Shared application state managed by Tauri.
pub struct AppState {
    pub webview2_version: String,
    pub profiles_dir: PathBuf,
    pub ephemeral_dir: PathBuf,
    pub downloads_dir: PathBuf,
    pub prefs_path: PathBuf,
    pub profiles_store_path: PathBuf,
    pub profile_names_path: PathBuf,
    pub preferences: Mutex<AppPreferences>,
    pub tabs: Mutex<Vec<TabEntry>>,
    pub downloads: Mutex<Vec<DownloadEntry>>,
    pub active_tab_id: Mutex<Option<String>>,
    pub shell_overlay_open: Mutex<bool>,
    pub profile_panel_open: Mutex<bool>,
    pub download_panel_open: Mutex<bool>,
}

impl AppState {
    pub fn new(
        webview2_version: String,
        profiles_dir: PathBuf,
        ephemeral_dir: PathBuf,
        downloads_dir: PathBuf,
        prefs_path: PathBuf,
        profiles_store_path: PathBuf,
        profile_names_path: PathBuf,
    ) -> Self {
        let loaded_prefs = load_preferences(&prefs_path);
        Self {
            webview2_version,
            profiles_dir,
            ephemeral_dir,
            downloads_dir,
            prefs_path,
            profiles_store_path,
            profile_names_path,
            preferences: Mutex::new(loaded_prefs),
            tabs: Mutex::new(Vec::new()),
            downloads: Mutex::new(Vec::new()),
            active_tab_id: Mutex::new(None),
            shell_overlay_open: Mutex::new(false),
            profile_panel_open: Mutex::new(false),
            download_panel_open: Mutex::new(false),
        }
    }

    pub fn snapshot(&self) -> ShellSnapshot {
        let tabs = self.tabs.lock().unwrap().clone();
        let active_awake_count = tabs.iter().filter(|t| !t.is_sleeping).count();
        let downloads = self.downloads.lock().unwrap().clone();
        let prefs = self.preferences.lock().unwrap().clone();
        let profiles = crate::profile::sync_profiles_store(
            &self.profiles_dir,
            &self.profiles_store_path,
            &self.profile_names_path,
            prefs.default_profile.as_deref(),
        );
        ShellSnapshot {
            webview2_version: self.webview2_version.clone(),
            profiles_dir: self.profiles_dir.display().to_string(),
            preferences: prefs,
            profiles,
            color_presets: crate::profile::PROFILE_COLOR_PRESETS
                .iter()
                .map(|c| (*c).to_string())
                .collect(),
            tabs,
            active_tab_id: self.active_tab_id.lock().unwrap().clone(),
            active_awake_count,
            downloads,
            profile_panel_open: *self.profile_panel_open.lock().unwrap(),
            download_panel_open: *self.download_panel_open.lock().unwrap(),
        }
    }
}

/// Loads saved UI Shell preferences from disk, falling back to defaults if missing or invalid.
pub fn load_preferences(path: &PathBuf) -> AppPreferences {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return AppPreferences::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Persists UI Shell preferences to disk.
pub fn save_preferences(path: &PathBuf, prefs: &AppPreferences) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(prefs).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// Extracts the first `http://` or `https://` URL argument passed to a second CLI launch.
pub fn extract_cli_url(argv: &[String]) -> Option<String> {
    argv.iter().skip(1).find_map(|arg| {
        let trimmed = arg.trim();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            url::Url::parse(trimmed).ok().map(|u| u.to_string())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn saves_and_loads_preferences_cleanly() {
        let path = std::env::temp_dir()
            .join(format!("roletab_prefs_{}", Uuid::new_v4().simple()))
            .join("prefs.json");
        assert_eq!(load_preferences(&path), AppPreferences::default());

        let updated = AppPreferences {
            theme: ThemeMode::Light,
            language: Language::En,
            default_profile: Some("admin".to_string()),
            group_downloads_by_profile: false,
        };
        save_preferences(&path, &updated).unwrap();
        assert_eq!(load_preferences(&path), updated);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn extracts_valid_cli_url_and_ignores_exe_path_or_flags() {
        let argv = vec![
            "C:\\RoleTab\\roletab.exe".to_string(),
            "--flag".to_string(),
            "https://app.example.com/login?role=admin".to_string(),
        ];
        assert_eq!(
            extract_cli_url(&argv),
            Some("https://app.example.com/login?role=admin".to_string())
        );

        let no_url = vec!["C:\\RoleTab\\roletab.exe".to_string(), "not-a-url".to_string()];
        assert_eq!(extract_cli_url(&no_url), None);
    }
}
