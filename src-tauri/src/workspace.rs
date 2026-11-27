use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::profile::default_color_for_name;
use crate::state::TabEntry;

/// A single tab entry inside a saved 1-click multi-role Workspace preset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTabSpec {
    pub profile_id: String,
    pub profile_name: String,
    pub profile_color: String,
    pub is_ephemeral: bool,
    pub url: String,
}

/// Named multi-role preset stored locally in `%APPDATA%\com.roletab.browser\workspaces.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePreset {
    pub id: String,
    pub name: String,
    pub tabs: Vec<WorkspaceTabSpec>,
    pub updated_at: u64,
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Loads saved Workspace presets from disk, returning empty list if missing or invalid.
pub fn load_workspaces(path: &Path) -> Vec<WorkspacePreset> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Persists Workspace presets cleanly to disk.
pub fn save_workspaces(path: &Path, presets: &[WorkspacePreset]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(presets).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// Converts currently open `TabEntry` items into normalized `WorkspaceTabSpec` entries.
pub fn specs_from_open_tabs(tabs: &[TabEntry]) -> Vec<WorkspaceTabSpec> {
    tabs.iter()
        .map(|tab| WorkspaceTabSpec {
            profile_id: if tab.is_ephemeral {
                "ephemeral".to_string()
            } else {
                tab.profile_id.clone()
            },
            profile_name: tab.profile_key.clone(),
            profile_color: if tab.profile_color.is_empty() {
                default_color_for_name(&tab.profile_key)
            } else {
                tab.profile_color.clone()
            },
            is_ephemeral: tab.is_ephemeral,
            url: tab.url.clone(),
        })
        .collect()
}

/// Validates and stores a new Workspace preset from `tabs`.
pub fn create_workspace_preset(
    workspaces_path: &Path,
    raw_name: &str,
    tabs: Vec<WorkspaceTabSpec>,
) -> Result<WorkspacePreset, String> {
    let name = raw_name.trim().to_string();
    if name.is_empty() {
        return Err("Nama workspace tidak boleh kosong".to_string());
    }
    if tabs.is_empty() {
        return Err("Workspace minimal memiliki 1 tab".to_string());
    }

    let mut presets = load_workspaces(workspaces_path);
    if presets.iter().any(|w| w.name.eq_ignore_ascii_case(&name)) {
        return Err(format!("Workspace \"{name}\" sudah ada"));
    }

    let preset = WorkspacePreset {
        id: Uuid::new_v4().simple().to_string(),
        name,
        tabs,
        updated_at: unix_now_secs(),
    };
    presets.insert(0, preset.clone());
    save_workspaces(workspaces_path, &presets)?;
    Ok(preset)
}

/// Deletes a Workspace preset by `workspace_id`.
pub fn delete_workspace_preset(workspaces_path: &Path, workspace_id: &str) -> Result<(), String> {
    let mut presets = load_workspaces(workspaces_path);
    let before = presets.len();
    presets.retain(|w| w.id != workspace_id);
    if presets.len() == before {
        return Err("Workspace tidak ditemukan".to_string());
    }
    save_workspaces(workspaces_path, &presets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_loads_and_deletes_multi_role_workspace_presets() {
        let base = std::env::temp_dir().join(format!("roletab_ws_{}", Uuid::new_v4().simple()));
        let ws_path = base.join("workspaces.json");

        let specs = vec![
            WorkspaceTabSpec {
                profile_id: "p-admin".into(),
                profile_name: "Admin".into(),
                profile_color: "#b91c1c".into(),
                is_ephemeral: false,
                url: "https://staging.local/admin".into(),
            },
            WorkspaceTabSpec {
                profile_id: "ephemeral".into(),
                profile_name: "Ephemeral".into(),
                profile_color: "#4b5563".into(),
                is_ephemeral: true,
                url: "https://staging.local/guest".into(),
            },
        ];

        assert!(create_workspace_preset(&ws_path, "   ", specs.clone()).is_err());
        assert!(create_workspace_preset(&ws_path, "Empty", vec![]).is_err());

        let created = create_workspace_preset(&ws_path, "Staging RBAC Flow", specs).unwrap();
        assert_eq!(created.tabs.len(), 2);
        assert!(create_workspace_preset(&ws_path, "staging rbac flow", created.tabs.clone()).is_err());

        let loaded = load_workspaces(&ws_path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "Staging RBAC Flow");

        delete_workspace_preset(&ws_path, &created.id).unwrap();
        assert!(load_workspaces(&ws_path).is_empty());

        let _ = std::fs::remove_dir_all(&base);
    }
}
