use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::state::TabEntry;

/// High-contrast preset palette for profile role badges (WCAG legible with white text).
pub const PROFILE_COLOR_PRESETS: &[&str] = &[
    "#b91c1c", // Crimson
    "#1d4ed8", // Royal Blue
    "#0f766e", // Teal
    "#6d28d9", // Violet
    "#b45309", // Amber Brown
    "#047857", // Emerald
    "#be185d", // Rose
    "#334155", // Slate
];

pub const EPHEMERAL_BADGE_COLOR: &str = "#4b5563";

/// Per-profile network proxy protocol (`direct` leaves OS/default network direct).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    #[default]
    Direct,
    Http,
    Socks5,
}

/// Validates and converts `(ProxyMode, raw_addr)` into an explicit proxy `url::Url` for WebView2.
pub fn build_profile_proxy_url(
    mode: &ProxyMode,
    raw_addr: Option<&str>,
) -> Result<Option<url::Url>, String> {
    if *mode == ProxyMode::Direct {
        return Ok(None);
    }
    let trimmed = raw_addr.unwrap_or("").trim();
    if trimmed.is_empty() {
        return Err("Alamat proxy wajib diisi (contoh: 127.0.0.1:8080)".to_string());
    }
    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .or_else(|| trimmed.strip_prefix("socks5://"))
        .or_else(|| trimmed.strip_prefix("socks5h://"))
        .unwrap_or(trimmed);

    let scheme = match mode {
        ProxyMode::Direct => return Ok(None),
        ProxyMode::Http => "http",
        ProxyMode::Socks5 => "socks5",
    };
    let candidate = format!("{scheme}://{without_scheme}");
    let parsed = url::Url::parse(&candidate)
        .map_err(|_| "Format alamat proxy tidak valid (gunakan host:port)".to_string())?;
    if parsed.host_str().is_none() || parsed.port().is_none() {
        return Err("Alamat proxy harus menyertakan host dan port (contoh: 127.0.0.1:8080)".to_string());
    }
    Ok(Some(parsed))
}

/// Persistent metadata record stored in `profiles.json` (`%APPDATA%\RoleTab\`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRecord {
    pub id: String,
    pub name: String,
    pub color: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub last_used_at: u64,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub proxy_mode: ProxyMode,
    #[serde(default)]
    pub proxy_address: Option<String>,
}

/// Real disk and runtime summary for a persistent profile shown in Profile Manager.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub id: String,
    pub key: String,
    pub name: String,
    pub color: String,
    pub path: String,
    pub size_bytes: u64,
    pub open_tab_count: usize,
    pub is_default: bool,
    pub last_used_at: u64,
    pub proxy_mode: ProxyMode,
    pub proxy_address: Option<String>,
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Sanitizes a profile name/key into a safe filesystem folder segment (also rejects path traversal).
pub fn sanitize_profile_key(key: &str) -> String {
    let cleaned: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Picks a deterministic high-contrast preset hex color for a profile name.
pub fn default_color_for_name(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    if lower == "admin" {
        return PROFILE_COLOR_PRESETS[0].to_string();
    }
    if lower == "user" {
        return PROFILE_COLOR_PRESETS[1].to_string();
    }
    let mut hash: usize = 0;
    for b in lower.as_bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(*b as usize);
    }
    PROFILE_COLOR_PRESETS[2 + (hash % (PROFILE_COLOR_PRESETS.len() - 2))].to_string()
}

/// Normalizes/validates a hex color against the contrast-safe preset palette (falls back safely).
pub fn normalize_profile_color(raw: Option<&str>, fallback_name: &str) -> String {
    if let Some(hex) = raw {
        let trimmed = hex.trim();
        if let Some(found) = PROFILE_COLOR_PRESETS
            .iter()
            .find(|preset| preset.eq_ignore_ascii_case(trimmed))
        {
            return (*found).to_string();
        }
    }
    default_color_for_name(fallback_name)
}

/// Resolves the isolated `.data_directory()` path for a persistent profile ID.
pub fn profile_data_dir(base_profiles_dir: &Path, profile_id: &str) -> PathBuf {
    base_profiles_dir.join(sanitize_profile_key(profile_id))
}

/// Loads legacy `sanitized_key -> display_name` map if present for smooth migration.
pub fn load_profile_names(path: &Path) -> HashMap<String, String> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Loads `profiles.json` from disk, returning empty vec if missing or unreadable.
pub fn load_profiles_store(path: &Path) -> Vec<ProfileRecord> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Persists `profiles.json` atomically-cleanly to disk.
pub fn save_profiles_store(path: &Path, records: &[ProfileRecord]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(records).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// Synchronizes `profiles.json` with real directories in `profiles_dir` (auto-importing any
/// legacy pre-Plan-4 folders cleanly without overwriting user-created UUID records).
pub fn sync_profiles_store(
    profiles_dir: &Path,
    profiles_store_path: &Path,
    legacy_names_path: &Path,
    legacy_default_key: Option<&str>,
) -> Vec<ProfileRecord> {
    let mut records = load_profiles_store(profiles_store_path);
    let legacy_names = load_profile_names(legacy_names_path);
    let mut changed = false;

    if let Ok(entries) = std::fs::read_dir(profiles_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                continue;
            }
            let folder = entry.file_name().to_string_lossy().to_string();
            let safe_id = sanitize_profile_key(&folder);
            if records.iter().any(|r| r.id == safe_id) {
                continue;
            }
            let name = legacy_names.get(&safe_id).cloned().unwrap_or(folder);
            let now = unix_now_secs();
            let is_default = legacy_default_key
                .map(sanitize_profile_key)
                .as_deref()
                == Some(safe_id.as_str())
                && !records.iter().any(|r| r.is_default);
            records.push(ProfileRecord {
                id: safe_id,
                color: default_color_for_name(&name),
                name,
                is_default,
                last_used_at: now,
                created_at: now,
                proxy_mode: ProxyMode::Direct,
                proxy_address: None,
            });
            changed = true;
        }
    }

    records.sort_by_key(|r| r.name.to_ascii_lowercase());
    if changed {
        let _ = save_profiles_store(profiles_store_path, &records);
    }
    records
}

/// Validates a new or renamed profile display name against existing `ProfileRecord`s.
pub fn validate_profile_name(
    raw_name: &str,
    existing: &[ProfileRecord],
    exclude_id: Option<&str>,
) -> Result<String, String> {
    let trimmed = raw_name.trim();
    if trimmed.is_empty() {
        return Err("Nama profil tidak boleh kosong".to_string());
    }
    if !trimmed.chars().any(|c| c.is_ascii_alphanumeric()) {
        return Err("Nama profil harus mengandung huruf atau angka".to_string());
    }
    if trimmed.eq_ignore_ascii_case("ephemeral") {
        return Err("Nama 'ephemeral' dicadangkan untuk tab anonim".to_string());
    }

    for rec in existing {
        if exclude_id == Some(rec.id.as_str()) {
            continue;
        }
        if rec.name.eq_ignore_ascii_case(trimmed) {
            return Err(format!("Nama profil \"{trimmed}\" sudah ada"));
        }
    }

    Ok(trimmed.to_string())
}

/// Creates a new UUID-backed `ProfileRecord` and its empty data directory.
pub fn create_profile_record(
    profiles_dir: &Path,
    profiles_store_path: &Path,
    legacy_names_path: &Path,
    raw_name: &str,
    color: Option<&str>,
) -> Result<ProfileRecord, String> {
    let mut records = sync_profiles_store(profiles_dir, profiles_store_path, legacy_names_path, None);
    let name = validate_profile_name(raw_name, &records, None)?;
    let now = unix_now_secs();
    let record = ProfileRecord {
        id: Uuid::new_v4().simple().to_string(),
        color: normalize_profile_color(color, &name),
        name,
        is_default: false,
        last_used_at: now,
        created_at: now,
        proxy_mode: ProxyMode::Direct,
        proxy_address: None,
    };

    let dir = profile_data_dir(profiles_dir, &record.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    records.push(record.clone());
    records.sort_by_key(|r| r.name.to_ascii_lowercase());
    save_profiles_store(profiles_store_path, &records)?;
    Ok(record)
}

/// Updates a profile's display name, badge color, and optional per-profile proxy settings in `profiles.json`.
pub fn update_profile_record(
    profiles_dir: &Path,
    profiles_store_path: &Path,
    legacy_names_path: &Path,
    profile_id: &str,
    raw_name: &str,
    color: Option<&str>,
    proxy_mode: Option<ProxyMode>,
    proxy_address: Option<String>,
) -> Result<ProfileRecord, String> {
    let mut records = sync_profiles_store(profiles_dir, profiles_store_path, legacy_names_path, None);
    let name = validate_profile_name(raw_name, &records, Some(profile_id))?;

    if let Some(ref mode) = proxy_mode {
        build_profile_proxy_url(mode, proxy_address.as_deref())?;
    }

    let target = records
        .iter_mut()
        .find(|r| r.id == profile_id)
        .ok_or_else(|| "Profil tidak ditemukan".to_string())?;
    target.name = name.clone();
    target.color = normalize_profile_color(color, &name);
    if let Some(mode) = proxy_mode {
        target.proxy_mode = mode.clone();
        target.proxy_address = if mode == ProxyMode::Direct {
            None
        } else {
            proxy_address.map(|a| a.trim().to_string()).filter(|a| !a.is_empty())
        };
    }
    let updated = target.clone();

    records.sort_by_key(|r| r.name.to_ascii_lowercase());
    save_profiles_store(profiles_store_path, &records)?;
    Ok(updated)
}

/// Marks one profile as default (clearing `is_default` on all others), or clears all when `None`.
pub fn set_default_profile_record(
    profiles_dir: &Path,
    profiles_store_path: &Path,
    legacy_names_path: &Path,
    profile_id: Option<&str>,
) -> Result<Vec<ProfileRecord>, String> {
    let mut records = sync_profiles_store(profiles_dir, profiles_store_path, legacy_names_path, None);
    if let Some(pid) = profile_id {
        if !records.iter().any(|r| r.id == pid) {
            return Err("Profil tidak ditemukan".to_string());
        }
    }
    for r in &mut records {
        r.is_default = profile_id == Some(r.id.as_str());
    }
    save_profiles_store(profiles_store_path, &records)?;
    Ok(records)
}

/// Updates `last_used_at` timestamp for `profile_id`.
pub fn touch_profile_last_used(profiles_store_path: &Path, profile_id: &str) {
    let mut records = load_profiles_store(profiles_store_path);
    if let Some(rec) = records.iter_mut().find(|r| r.id == profile_id) {
        rec.last_used_at = unix_now_secs();
        let _ = save_profiles_store(profiles_store_path, &records);
    }
}

/// Removes a deleted profile from `profiles.json`.
pub fn remove_profile_record(profiles_store_path: &Path, profile_id: &str) -> Result<(), String> {
    let mut records = load_profiles_store(profiles_store_path);
    let before = records.len();
    records.retain(|r| r.id != profile_id);
    if records.len() != before {
        save_profiles_store(profiles_store_path, &records)?;
    }
    Ok(())
}

/// Finds a profile by ID or case-insensitive name.
pub fn find_profile_record(
    profiles_dir: &Path,
    profiles_store_path: &Path,
    legacy_names_path: &Path,
    id_or_name: &str,
) -> Option<ProfileRecord> {
    let records = sync_profiles_store(profiles_dir, profiles_store_path, legacy_names_path, None);
    records
        .iter()
        .find(|r| r.id == id_or_name || r.name.eq_ignore_ascii_case(id_or_name))
        .cloned()
}

/// Computes total physical byte size of a directory recursively.
pub fn dir_size_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0_u64;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_file() {
            total = total.saturating_add(meta.len());
        } else if meta.is_dir() {
            total = total.saturating_add(dir_size_bytes(&entry.path()));
        }
    }
    total
}

/// Clears browser cache/cookies files inside a closed profile's data directory cleanly.
pub fn clear_closed_profile_disk_data(profile_dir: &Path) -> Result<(), String> {
    let eb_default = profile_dir.join("EBWebView").join("Default");
    if !eb_default.exists() {
        return Ok(());
    }
    for sub in [
        "Cache",
        "Code Cache",
        "GPUCache",
        "DawnCache",
        "DawnGraphiteCache",
        "Service Worker",
        "IndexedDB",
        "Local Storage",
        "Session Storage",
    ] {
        let target = eb_default.join(sub);
        if target.exists() {
            let _ = std::fs::remove_dir_all(&target);
        }
    }
    for file in ["Cookies", "Cookies-journal", "Network Persistent State"] {
        let target = eb_default.join(file);
        if target.exists() {
            let _ = std::fs::remove_file(&target);
        }
    }
    Ok(())
}

/// Builds real per-profile summaries (disk size, open tab counts, default flag) for Profile Manager.
pub fn build_profile_summaries(
    profiles_dir: &Path,
    profiles_store_path: &Path,
    legacy_names_path: &Path,
    open_tabs: &[TabEntry],
    legacy_default_key: Option<&str>,
) -> Vec<ProfileSummary> {
    let records = sync_profiles_store(
        profiles_dir,
        profiles_store_path,
        legacy_names_path,
        legacy_default_key,
    );

    records
        .into_iter()
        .map(|rec| {
            let dir = profile_data_dir(profiles_dir, &rec.id);
            let open_tab_count = open_tabs
                .iter()
                .filter(|t| !t.is_ephemeral && t.profile_id == rec.id)
                .count();
            ProfileSummary {
                id: rec.id.clone(),
                key: rec.id,
                name: rec.name,
                color: rec.color,
                path: dir.display().to_string(),
                size_bytes: dir_size_bytes(&dir),
                open_tab_count,
                is_default: rec.is_default,
                last_used_at: rec.last_used_at,
                proxy_mode: rec.proxy_mode,
                proxy_address: rec.proxy_address,
            }
        })
        .collect()
}

/// Attempts to delete a profile directory with retries to handle brief Windows WebView2 file locks.
pub fn delete_dir_with_retry(path: &Path, max_attempts: usize, delay: Duration) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    let mut last_err = String::new();
    for attempt in 0..max_attempts {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_err = err.to_string();
                if attempt + 1 < max_attempts {
                    std::thread::sleep(delay);
                }
            }
        }
    }

    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_profile_key_prevents_path_traversal() {
        assert_eq!(sanitize_profile_key("Admin Utama!"), "admin_utama");
        assert_eq!(sanitize_profile_key("../secret"), "secret");
        assert_eq!(sanitize_profile_key("   "), "default");
    }

    #[test]
    fn profile_data_dir_joins_sanitized_segment() {
        let base = PathBuf::from("C:/Users/qa/AppData/Local/RoleTab/profiles");
        assert_eq!(
            profile_data_dir(&base, "Super Admin"),
            base.join("super_admin")
        );
    }

    #[test]
    fn syncs_legacy_folders_into_uuid_store_and_preserves_display_names() {
        let base = std::env::temp_dir().join(format!("roletab_sync_{}", Uuid::new_v4().simple()));
        let profiles_dir = base.join("profiles");
        let store_path = base.join("profiles.json");
        let legacy_path = base.join("profile_names.json");
        std::fs::create_dir_all(profiles_dir.join("super_admin")).unwrap();
        std::fs::write(
            &legacy_path,
            serde_json::to_string(&HashMap::from([(
                "super_admin".to_string(),
                "Super Admin".to_string(),
            )]))
            .unwrap(),
        )
        .unwrap();

        let synced = sync_profiles_store(&profiles_dir, &store_path, &legacy_path, Some("super_admin"));
        assert_eq!(synced.len(), 1);
        assert_eq!(synced[0].id, "super_admin");
        assert_eq!(synced[0].name, "Super Admin");
        assert!(synced[0].is_default);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn creates_renames_and_recolors_profile_without_moving_folder() {
        let base = std::env::temp_dir().join(format!("roletab_crud_{}", Uuid::new_v4().simple()));
        let profiles_dir = base.join("profiles");
        let store_path = base.join("profiles.json");
        let legacy_path = base.join("profile_names.json");
        std::fs::create_dir_all(&profiles_dir).unwrap();

        let created = create_profile_record(
            &profiles_dir,
            &store_path,
            &legacy_path,
            "Kasir Pagi",
            Some("#0f766e"),
        )
        .unwrap();
        assert_eq!(created.name, "Kasir Pagi");
        assert_eq!(created.color, "#0f766e");
        assert!(profile_data_dir(&profiles_dir, &created.id).exists());

        // Duplicate name rejected
        assert!(create_profile_record(
            &profiles_dir,
            &store_path,
            &legacy_path,
            "kasir pagi",
            None
        )
        .is_err());

        // Rename, recolor & configure HTTP proxy keeping the exact same UUID folder path
        let updated = update_profile_record(
            &profiles_dir,
            &store_path,
            &legacy_path,
            &created.id,
            "Kasir Malam",
            Some("#6d28d9"),
            Some(ProxyMode::Http),
            Some("127.0.0.1:8080".into()),
        )
        .unwrap();
        assert_eq!(updated.id, created.id);
        assert_eq!(updated.name, "Kasir Malam");
        assert_eq!(updated.color, "#6d28d9");
        assert_eq!(updated.proxy_mode, ProxyMode::Http);
        assert_eq!(updated.proxy_address.as_deref(), Some("127.0.0.1:8080"));
        assert!(build_profile_proxy_url(&ProxyMode::Http, Some("bad-no-port")).is_err());
        assert!(profile_data_dir(&profiles_dir, &created.id).exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn validate_profile_name_rejects_empty_reserved_and_duplicates() {
        let existing = vec![
            ProfileRecord {
                id: "id1".into(),
                name: "Admin".into(),
                color: "#b91c1c".into(),
                is_default: false,
                last_used_at: 0,
                created_at: 0,
                proxy_mode: ProxyMode::Direct,
                proxy_address: None,
            },
            ProfileRecord {
                id: "id2".into(),
                name: "Kasir".into(),
                color: "#0f766e".into(),
                is_default: false,
                last_used_at: 0,
                created_at: 0,
                proxy_mode: ProxyMode::Direct,
                proxy_address: None,
            },
        ];
        assert!(validate_profile_name("   ", &existing, None).is_err());
        assert!(validate_profile_name("---", &existing, None).is_err());
        assert!(validate_profile_name("Ephemeral", &existing, None).is_err());
        assert!(validate_profile_name("admin", &existing, None).is_err());
        // Updating own record keeps current name valid, but rejects collision with another record
        assert_eq!(
            validate_profile_name("Admin", &existing, Some("id1")).unwrap(),
            "Admin"
        );
        assert!(validate_profile_name("kasir", &existing, Some("id1")).is_err());
    }

    #[test]
    fn dir_size_bytes_and_profile_summaries_report_real_values() {
        let base = std::env::temp_dir().join(format!("roletab_sum_{}", Uuid::new_v4().simple()));
        let profiles_dir = base.join("profiles");
        let store_path = base.join("profiles.json");
        let legacy_path = base.join("profile_names.json");
        std::fs::create_dir_all(&profiles_dir).unwrap();

        let rec =
            create_profile_record(&profiles_dir, &store_path, &legacy_path, "Admin", Some("#b91c1c"))
                .unwrap();
        set_default_profile_record(&profiles_dir, &store_path, &legacy_path, Some(&rec.id))
            .unwrap();

        let admin_dir = profile_data_dir(&profiles_dir, &rec.id);
        std::fs::create_dir_all(admin_dir.join("EBWebView/Default/Cache")).unwrap();
        std::fs::write(admin_dir.join("EBWebView/Default/Cookies"), vec![1_u8; 256]).unwrap();
        std::fs::write(
            admin_dir.join("EBWebView/Default/Cache/blob.bin"),
            vec![2_u8; 512],
        )
        .unwrap();

        let tabs = vec![TabEntry {
            id: "t1".into(),
            profile_id: rec.id.clone(),
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
            title: "Dash".into(),
            url: "http://localhost".into(),
        }];

        let summaries =
            build_profile_summaries(&profiles_dir, &store_path, &legacy_path, &tabs, None);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].name, "Admin");
        assert_eq!(summaries[0].size_bytes, 768);
        assert_eq!(summaries[0].open_tab_count, 1);
        assert!(summaries[0].is_default);

        clear_closed_profile_disk_data(&admin_dir).unwrap();
        assert_eq!(dir_size_bytes(&admin_dir), 0);

        remove_profile_record(&store_path, &rec.id).unwrap();
        assert!(load_profiles_store(&store_path).is_empty());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn delete_dir_with_retry_removes_existing_folder_and_ignores_missing() {
        let base = std::env::temp_dir().join(format!("roletab_prof_{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(base.join("nested")).unwrap();
        std::fs::write(base.join("nested/cookie.txt"), b"ok").unwrap();

        delete_dir_with_retry(&base, 3, Duration::from_millis(10)).unwrap();
        assert!(!base.exists());

        delete_dir_with_retry(&base, 2, Duration::from_millis(10)).unwrap();
    }
}
