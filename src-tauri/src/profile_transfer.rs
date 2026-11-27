use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::profile::{
    load_profiles_store, normalize_profile_color, profile_data_dir, save_profiles_store,
    sync_profiles_store, validate_profile_name, ProfileRecord, ProxyMode,
};

const MANIFEST_ENTRY_NAME: &str = "roletab-profile.json";
const DATA_PREFIX: &str = "data/";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportedProfileManifest {
    id: String,
    name: String,
    color: String,
    #[serde(default)]
    proxy_mode: ProxyMode,
    #[serde(default)]
    proxy_address: Option<String>,
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn append_dir_to_zip(
    writer: &mut ZipWriter<File>,
    root_dir: &Path,
    cur_dir: &Path,
    options: SimpleFileOptions,
) -> Result<(), String> {
    let entries = fs::read_dir(cur_dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path.strip_prefix(root_dir).map_err(|e| e.to_string())?;
        let rel_slash = rel.to_string_lossy().replace('\\', "/");
        if path.is_dir() {
            append_dir_to_zip(writer, root_dir, &path, options)?;
        } else if path.is_file() {
            let entry_name = format!("{DATA_PREFIX}{rel_slash}");
            writer
                .start_file(entry_name, options)
                .map_err(|e| e.to_string())?;
            let mut f = File::open(&path).map_err(|e| e.to_string())?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            writer.write_all(&buf).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Packages `record` metadata + all files inside `profile_dir` into a structured `.zip` archive.
pub fn export_profile_dir_to_zip(
    record: &ProfileRecord,
    profile_dir: &Path,
    dest_zip: &Path,
) -> Result<(), String> {
    if let Some(parent) = dest_zip.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = File::create(dest_zip).map_err(|e| e.to_string())?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let manifest = ExportedProfileManifest {
        id: record.id.clone(),
        name: record.name.clone(),
        color: record.color.clone(),
        proxy_mode: record.proxy_mode.clone(),
        proxy_address: record.proxy_address.clone(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    writer
        .start_file(MANIFEST_ENTRY_NAME, options)
        .map_err(|e| e.to_string())?;
    writer
        .write_all(&manifest_bytes)
        .map_err(|e| e.to_string())?;

    if profile_dir.exists() {
        append_dir_to_zip(&mut writer, profile_dir, profile_dir, options)?;
    }

    writer.finish().map_err(|e| e.to_string())?;
    Ok(())
}

fn pick_unique_import_name(base_name: &str, existing: &[ProfileRecord]) -> String {
    let trimmed = if base_name.trim().is_empty() {
        "Imported Role"
    } else {
        base_name.trim()
    };
    if validate_profile_name(trimmed, existing, None).is_ok() {
        return trimmed.to_string();
    }
    for i in 1..=999 {
        let candidate = if i == 1 {
            format!("{trimmed} (Import)")
        } else {
            format!("{trimmed} (Import {i})")
        };
        if validate_profile_name(&candidate, existing, None).is_ok() {
            return candidate;
        }
    }
    format!("Profile {}", Uuid::new_v4().simple())
}

/// Imports a RoleTab `.zip` profile archive into `profiles_dir`, assigning a fresh UUID if
/// colliding with an existing profile ID and preventing Zip Slip path traversal.
pub fn import_profile_zip_to_dir(
    profiles_dir: &Path,
    profiles_store_path: &Path,
    legacy_names_path: &Path,
    src_zip: &Path,
) -> Result<ProfileRecord, String> {
    let file = File::open(src_zip).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("Arsip ZIP tidak valid: {e}"))?;

    let mut manifest_opt: Option<ExportedProfileManifest> = None;
    if let Ok(mut mf) = archive.by_name(MANIFEST_ENTRY_NAME) {
        let mut raw = String::new();
        let _ = mf.read_to_string(&mut raw);
        manifest_opt = serde_json::from_str(&raw).ok();
    }

    let records = sync_profiles_store(profiles_dir, profiles_store_path, legacy_names_path, None);
    let fallback_name = src_zip
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported");

    let (raw_id, raw_name, raw_color, proxy_mode, proxy_address) = match manifest_opt {
        Some(m) => (m.id, m.name, Some(m.color), m.proxy_mode, m.proxy_address),
        None => (
            String::new(),
            fallback_name.to_string(),
            None,
            ProxyMode::Direct,
            None,
        ),
    };

    let target_id = if raw_id.trim().is_empty()
        || records.iter().any(|r| r.id == raw_id)
        || profile_data_dir(profiles_dir, &raw_id).exists()
    {
        Uuid::new_v4().simple().to_string()
    } else {
        raw_id
    };

    let final_name = pick_unique_import_name(&raw_name, &records);
    let final_color = normalize_profile_color(raw_color.as_deref(), &final_name);
    let target_dir = profile_data_dir(profiles_dir, &target_id);
    fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;

    for idx in 0..archive.len() {
        let mut entry = archive.by_index(idx).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let Some(enclosed) = entry.enclosed_name() else {
            return Err("Entri ZIP mengandung path traversal tidak aman".to_string());
        };
        let enclosed_str = enclosed.to_string_lossy().replace('\\', "/");
        if enclosed_str == MANIFEST_ENTRY_NAME {
            continue;
        }
        let rel = enclosed_str.strip_prefix(DATA_PREFIX).unwrap_or(&enclosed_str);
        if rel.is_empty() {
            continue;
        }
        let out_path = target_dir.join(rel);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out_file = File::create(&out_path).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out_file).map_err(|e| e.to_string())?;
    }

    let now = unix_now_secs();
    let imported = ProfileRecord {
        id: target_id,
        name: final_name,
        color: final_color,
        is_default: false,
        last_used_at: now,
        created_at: now,
        proxy_mode,
        proxy_address,
    };

    let mut latest = load_profiles_store(profiles_store_path);
    if latest.is_empty() {
        latest = records;
    }
    latest.push(imported.clone());
    latest.sort_by_key(|r| r.name.to_ascii_lowercase());
    save_profiles_store(profiles_store_path, &latest)?;
    Ok(imported)
}

/// Opens native Windows Open File dialog for `.zip` profile archive import (`None` if cancelled).
#[cfg(windows)]
pub fn pick_open_zip_path() -> Option<PathBuf> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let mut buf = vec![0_u16; 1024];
    let filter: Vec<u16> = "RoleTab Profile (*.zip)\0*.zip\0All Files (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "Import Profil RoleTab\0".encode_utf16().collect();

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };

    let ok = unsafe { GetOpenFileNameW(&mut ofn).as_bool() };
    if ok {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    } else {
        None
    }
}

#[cfg(not(windows))]
pub fn pick_open_zip_path() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{create_profile_record, profile_data_dir};

    #[test]
    fn exports_and_imports_profile_zip_preserving_data_and_resolving_id_collision() {
        let base = std::env::temp_dir().join(format!("roletab_zip_{}", Uuid::new_v4().simple()));
        let profiles_dir = base.join("profiles");
        let store_path = base.join("profiles.json");
        let names_path = base.join("profile_names.json");
        std::fs::create_dir_all(&profiles_dir).unwrap();

        let rec =
            create_profile_record(&profiles_dir, &store_path, &names_path, "Kasir", Some("#0f766e"))
                .unwrap();
        let src_dir = profile_data_dir(&profiles_dir, &rec.id);
        std::fs::create_dir_all(src_dir.join("EBWebView/Default")).unwrap();
        std::fs::write(
            src_dir.join("EBWebView/Default/Cookies"),
            b"session_token=kasir_123",
        )
        .unwrap();

        let zip_path = base.join("kasir_export.zip");
        export_profile_dir_to_zip(&rec, &src_dir, &zip_path).unwrap();
        assert!(zip_path.exists());

        // Import back into the SAME store where `rec.id` and `"Kasir"` already exist
        let imported =
            import_profile_zip_to_dir(&profiles_dir, &store_path, &names_path, &zip_path).unwrap();
        assert_ne!(imported.id, rec.id);
        assert_eq!(imported.name, "Kasir (Import)");
        assert_eq!(imported.color, "#0f766e");

        let imported_cookies =
            std::fs::read(profile_data_dir(&profiles_dir, &imported.id).join("EBWebView/Default/Cookies"))
                .unwrap();
        assert_eq!(imported_cookies, b"session_token=kasir_123");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn import_rejects_zip_slip_traversal_entries() {
        let base =
            std::env::temp_dir().join(format!("roletab_zipslip_{}", Uuid::new_v4().simple()));
        let profiles_dir = base.join("profiles");
        let store_path = base.join("profiles.json");
        let names_path = base.join("profile_names.json");
        std::fs::create_dir_all(&profiles_dir).unwrap();

        let bad_zip = base.join("evil.zip");
        {
            let file = File::create(&bad_zip).unwrap();
            let mut w = ZipWriter::new(file);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            w.start_file("../../escaped.txt", opts).unwrap();
            w.write_all(b"evil").unwrap();
            w.finish().unwrap();
        }

        assert!(import_profile_zip_to_dir(&profiles_dir, &store_path, &names_path, &bad_zip).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
