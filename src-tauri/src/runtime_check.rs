/// Minimum supported Edge WebView2 major version.
#[cfg(any(windows, test))]
pub const MIN_WEBVIEW2_MAJOR: u32 = 109;

/// Parses the major version number from a WebView2 version string (e.g., `"131.0.2903.112"`).
#[cfg(any(windows, test))]
pub fn parse_webview2_major(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse::<u32>().ok()
}

/// Validates that an installed WebView2 version string satisfies `min_major`.
#[cfg(any(windows, test))]
pub fn validate_webview2_version(version: &str, min_major: u32) -> Result<u32, String> {
    let major = parse_webview2_major(version)
        .ok_or_else(|| format!("Versi WebView2 Runtime tidak terbaca: '{version}'"))?;
    if major < min_major {
        return Err(format!(
            "Versi WebView2 Runtime ({version}) terlalu lama. Minimal versi {min_major}.x."
        ));
    }
    Ok(major)
}

/// Checks whether Microsoft Edge WebView2 Runtime is installed and meets `MIN_WEBVIEW2_MAJOR`.
#[cfg(windows)]
pub fn ensure_webview2_runtime() -> Result<String, String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::GetAvailableCoreWebView2BrowserVersionString;
    use webview2_com::take_pwstr;
    use windows_core::{PCWSTR, PWSTR};

    let mut raw_version = PWSTR::null();
    let hr = unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut raw_version) };
    if hr.is_err() || raw_version.is_null() {
        return Err(
            "Microsoft Edge WebView2 Runtime tidak ditemukan.\nSilakan pasang Evergreen WebView2 Runtime dari Microsoft lalu jalankan ulang RoleTab."
                .to_string(),
        );
    }

    let version = take_pwstr(raw_version);
    validate_webview2_version(&version, MIN_WEBVIEW2_MAJOR)?;
    Ok(version)
}

#[cfg(not(windows))]
pub fn ensure_webview2_runtime() -> Result<String, String> {
    Ok("non-windows".to_string())
}

/// Displays a native startup error dialog on Windows when WebView2 Runtime is missing or outdated.
#[cfg(windows)]
pub fn show_startup_error_dialog(message: &str) {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let title = HSTRING::from("RoleTab - WebView2 Runtime Diperlukan");
    let body = HSTRING::from(format!(
        "{message}\n\nUnduh di: https://developer.microsoft.com/en-us/microsoft-edge/webview2/"
    ));
    unsafe {
        let _ = MessageBoxW(None, &body, &title, MB_OK | MB_ICONERROR);
    }
}

#[cfg(not(windows))]
pub fn show_startup_error_dialog(message: &str) {
    eprintln!("{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_validates_webview2_versions() {
        assert_eq!(parse_webview2_major("131.0.2903.112"), Some(131));
        assert_eq!(parse_webview2_major(" 109.0.1518.78 "), Some(109));
        assert_eq!(parse_webview2_major(""), None);
        assert_eq!(parse_webview2_major("abc.1.2"), None);

        assert_eq!(validate_webview2_version("131.0.2903.112", MIN_WEBVIEW2_MAJOR), Ok(131));
        assert!(validate_webview2_version("95.0.1020.44", MIN_WEBVIEW2_MAJOR).is_err());
        assert!(validate_webview2_version("invalid", MIN_WEBVIEW2_MAJOR).is_err());
    }
}
