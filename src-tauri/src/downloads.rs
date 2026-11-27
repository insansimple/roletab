use std::path::{Path, PathBuf};

/// Sanitizes a profile name so it is valid as a single Windows folder name.
/// Replaces illegal characters (`\ / : * ? " < > |`) and trailing dots/spaces.
pub fn sanitize_windows_folder_name(raw: &str) -> String {
    let replaced: String = raw
        .trim()
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            other => other,
        })
        .collect();
    let trimmed = replaced.trim_matches(|c| c == ' ' || c == '.');
    if trimmed.is_empty() {
        "Default".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Resolves the destination directory for a download, optionally grouped under `RoleTab/<Profile>`.
pub fn resolve_download_target_dir(
    base_downloads_dir: &Path,
    profile_name: &str,
    group_by_profile: bool,
) -> PathBuf {
    if group_by_profile {
        base_downloads_dir
            .join("RoleTab")
            .join(sanitize_windows_folder_name(profile_name))
    } else {
        base_downloads_dir.to_path_buf()
    }
}

/// Resolves a collision-free file path in `target_dir` (`report (1).pdf`, `report (2).pdf`, etc.).
pub fn resolve_unique_download_path(target_dir: &Path, raw_file_name: &str) -> PathBuf {
    let safe_name = Path::new(raw_file_name)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("download.bin");

    let candidate = target_dir.join(safe_name);
    if !candidate.exists() {
        return candidate;
    }

    let path_obj = Path::new(safe_name);
    let stem = path_obj
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("download");
    let ext = path_obj.extension().and_then(|s| s.to_str());

    for index in 1..=9999_u32 {
        let next_name = match ext {
            Some(e) if !e.is_empty() => format!("{stem} ({index}).{e}"),
            _ => format!("{stem} ({index})"),
        };
        let next_path = target_dir.join(next_name);
        if !next_path.exists() {
            return next_path;
        }
    }

    target_dir.join(format!("{stem}_{}", uuid::Uuid::new_v4().simple()))
}

/// Reveals a downloaded file in Windows Explorer (`explorer /select,<path>`).
#[cfg(windows)]
pub fn reveal_in_explorer(path: &Path) -> Result<(), String> {
    if path.exists() {
        let arg = format!("/select,{}", path.display());
        std::process::Command::new("explorer.exe")
            .arg(arg)
            .spawn()
            .map_err(|e| format!("Gagal membuka Explorer: {e}"))?;
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        if parent.exists() {
            std::process::Command::new("explorer.exe")
                .arg(parent)
                .spawn()
                .map_err(|e| format!("Gagal membuka folder: {e}"))?;
            return Ok(());
        }
    }
    Err("File unduhan tidak ditemukan di disk".to_string())
}

#[cfg(not(windows))]
pub fn reveal_in_explorer(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn sanitizes_illegal_windows_folder_characters() {
        assert_eq!(sanitize_windows_folder_name("Admin: QA/Prod*"), "Admin_ QA_Prod_");
        assert_eq!(sanitize_windows_folder_name("   ...  "), "Default");
        assert_eq!(sanitize_windows_folder_name("Kasir 01"), "Kasir 01");
    }

    #[test]
    fn groups_downloads_under_roletab_profile_folder_when_enabled() {
        let base = PathBuf::from("C:/Users/qa/Downloads");
        assert_eq!(
            resolve_download_target_dir(&base, "Super:Admin", true),
            base.join("RoleTab").join("Super_Admin")
        );
        assert_eq!(
            resolve_download_target_dir(&base, "Super:Admin", false),
            base
        );
    }

    #[test]
    fn resolves_unique_download_filenames_without_overwriting() {
        let base = std::env::temp_dir().join(format!("roletab_dl_{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&base).unwrap();

        let p0 = resolve_unique_download_path(&base, "laporan.pdf");
        assert_eq!(p0.file_name().unwrap(), "laporan.pdf");
        std::fs::write(&p0, b"first").unwrap();

        let p1 = resolve_unique_download_path(&base, "laporan.pdf");
        assert_eq!(p1.file_name().unwrap(), "laporan (1).pdf");
        std::fs::write(&p1, b"second").unwrap();

        let p2 = resolve_unique_download_path(&base, "laporan.pdf");
        assert_eq!(p2.file_name().unwrap(), "laporan (2).pdf");

        let no_ext0 = resolve_unique_download_path(&base, "README");
        std::fs::write(&no_ext0, b"a").unwrap();
        let no_ext1 = resolve_unique_download_path(&base, "README");
        assert_eq!(no_ext1.file_name().unwrap(), "README (1)");

        let _ = std::fs::remove_dir_all(&base);
    }
}
