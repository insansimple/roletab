use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

/// Starts an embedded localhost HTTP server on an ephemeral port (`127.0.0.1:0`)
/// serving self-contained test pages for validating session isolation, shortcuts,
/// `TrySuspendAsync`, `target="_blank"`, and `window.open` OAuth flows.
pub fn start_test_server() -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();

    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || handle_connection(stream));
        }
    });

    Ok(port)
}

fn handle_connection(mut stream: TcpStream) {
    let mut buffer = [0_u8; 2048];
    let bytes_read = match stream.read(&mut buffer) {
        Ok(n) if n > 0 => n,
        _ => return,
    };

    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let path = parse_request_path(&request);

    let (status, content_type, body) = match path {
        "/" => ("200 OK", "text/html; charset=utf-8", MAIN_TEST_HTML),
        "/target-blank" => ("200 OK", "text/html; charset=utf-8", TARGET_BLANK_HTML),
        "/oauth-popup" => ("200 OK", "text/html; charset=utf-8", OAUTH_POPUP_HTML),
        _ => ("404 Not Found", "text/plain; charset=utf-8", "Not Found"),
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

pub(crate) fn parse_request_path(request: &str) -> &str {
    let first_line = request.lines().next().unwrap_or("");
    let raw_target = first_line.split_whitespace().nth(1).unwrap_or("/");
    raw_target.split('?').next().unwrap_or("/")
}

const MAIN_TEST_HTML: &str = r#"<!doctype html>
<html lang="id">
<head>
  <meta charset="UTF-8" />
  <title>RoleTab Spike — Test Fixture</title>
  <style>
    body { font-family: system-ui, sans-serif; margin: 24px; background: #0f172a; color: #e2e8f0; }
    .grid { display: grid; gap: 16px; grid-template-columns: repeat(auto-fit, minmax(290px, 1fr)); }
    .card { background: #1e293b; border: 1px solid #334155; border-radius: 10px; padding: 16px; }
    h2 { margin: 0 0 10px; font-size: 16px; color: #38bdf8; }
    button, a.btn { background: #2563eb; color: #fff; border: 0; padding: 8px 12px; border-radius: 6px; cursor: pointer; text-decoration: none; display: inline-block; font-size: 13px; margin: 4px 4px 0 0; }
    button.secondary { background: #475569; }
    input { width: 100%; box-sizing: border-box; padding: 8px; border-radius: 6px; border: 1px solid #475569; background: #0f172a; color: #fff; margin-top: 6px; }
    pre { background: #020617; padding: 10px; border-radius: 6px; font-size: 12px; overflow-x: auto; margin: 8px 0 0; }
  </style>
</head>
<body>
  <h1>RoleTab Spike — Halaman Uji WebView2</h1>
  <div class="grid">
    <div class="card">
      <h2>1. Isolasi Sesi (Cookie &amp; LocalStorage)</h2>
      <input id="roleInput" placeholder="Masukkan nama role / user (mis. admin-1)" />
      <div>
        <button onclick="saveSession()">Simpan Sesi</button>
        <button class="secondary" onclick="clearSession()">Hapus Sesi</button>
        <button class="secondary" onclick="refreshState()">Refresh</button>
      </div>
      <pre id="sessionOut"></pre>
    </div>

    <div class="card">
      <h2>2. Fokus Input &amp; Shortcut + Tauri IPC Check</h2>
      <input id="focusInput" placeholder="Klik di sini lalu tekan Ctrl+T / Ctrl+W / Ctrl+L / F12" />
      <pre id="ipcOut"></pre>
    </div>

    <div class="card">
      <h2>3. Navigasi target="_blank" &amp; OAuth Popup</h2>
      <a class="btn" href="/target-blank" target="_blank">Buka target="_blank"</a>
      <button onclick="openOAuthPopup()">Uji OAuth window.open()</button>
      <pre id="oauthOut">Belum ada pesan OAuth.</pre>
    </div>

    <div class="card">
      <h2>4. Uji TrySuspendAsync &amp; Resume</h2>
      <p>JS Heartbeat counter (berhenti saat tab di-suspend):</p>
      <pre id="heartbeatOut">0</pre>
    </div>
  </div>

  <script>
    function refreshState() {
      const ls = localStorage.getItem('roletab_session') || '(kosong)';
      const ck = document.cookie || '(kosong)';
      document.getElementById('sessionOut').textContent =
        `localStorage: ${ls}\ndocument.cookie: ${ck}`;
      document.getElementById('ipcOut').textContent =
        `window.__TAURI__ = ${typeof window.__TAURI__ === 'undefined' ? 'undefined (AMAN)' : 'EXPOSED (BAHAYA!)'}`;
    }

    function saveSession() {
      const val = document.getElementById('roleInput').value.trim() || ('user-' + Math.floor(Math.random() * 1000));
      localStorage.setItem('roletab_session', val);
      document.cookie = `roletab_cookie=${encodeURIComponent(val)}; path=/; SameSite=Lax`;
      refreshState();
    }

    function clearSession() {
      localStorage.removeItem('roletab_session');
      document.cookie = 'roletab_cookie=; expires=Thu, 01 Jan 1970 00:00:00 GMT; path=/';
      refreshState();
    }

    function openOAuthPopup() {
      document.getElementById('oauthOut').textContent = 'Membuka popup OAuth...';
      window.open('/oauth-popup', 'roletab_oauth', 'width=480,height=560');
    }

    window.addEventListener('message', (event) => {
      document.getElementById('oauthOut').textContent =
        'Diterima dari popup: ' + JSON.stringify(event.data);
    });

    let ticks = 0;
    setInterval(() => {
      ticks += 1;
      document.getElementById('heartbeatOut').textContent =
        `ticks=${ticks} | time=${new Date().toLocaleTimeString()}`;
    }, 1000);

    refreshState();
  </script>
</body>
</html>
"#;

const TARGET_BLANK_HTML: &str = r#"<!doctype html>
<html lang="id">
<head>
  <meta charset="UTF-8" />
  <title>Target Blank Inherited</title>
  <style>
    body { font-family: system-ui, sans-serif; margin: 24px; background: #0f172a; color: #e2e8f0; }
    pre { background: #1e293b; padding: 12px; border-radius: 8px; }
  </style>
</head>
<body>
  <h2>Halaman target="_blank" berhasil dibuka</h2>
  <pre id="out"></pre>
  <script>
    document.getElementById('out').textContent =
      `localStorage: ${localStorage.getItem('roletab_session') || '(kosong)'}\n` +
      `document.cookie: ${document.cookie || '(kosong)'}\n` +
      `window.__TAURI__: ${typeof window.__TAURI__}`;
  </script>
</body>
</html>
"#;

const OAUTH_POPUP_HTML: &str = r#"<!doctype html>
<html lang="id">
<head>
  <meta charset="UTF-8" />
  <title>Simulasi OAuth Login Popup</title>
  <style>
    body { font-family: system-ui, sans-serif; margin: 24px; background: #1e1b4b; color: #e0e7ff; }
    button { background: #10b981; color: #052e16; font-weight: 600; border: 0; padding: 10px 14px; border-radius: 6px; cursor: pointer; }
    pre { background: #0f172a; padding: 10px; border-radius: 6px; }
  </style>
</head>
<body>
  <h2>Simulasi OAuth / SSO Popup</h2>
  <pre id="status"></pre>
  <button onclick="sendToken()">Kirim Token ke window.opener &amp; Tutup Popup</button>
  <script>
    const hasOpener = Boolean(window.opener);
    document.getElementById('status').textContent =
      `window.opener tersedia: ${hasOpener}\n` +
      `localStorage sesi: ${localStorage.getItem('roletab_session') || '(kosong)'}\n` +
      `cookie sesi: ${document.cookie || '(kosong)'}`;

    function sendToken() {
      if (!window.opener) {
        alert('window.opener NULL! Handshake OAuth gagal.');
        return;
      }
      window.opener.postMessage({
        type: 'oauth-success',
        token: 'mock-jwt-' + Date.now(),
        inheritedRole: localStorage.getItem('roletab_session') || 'guest'
      }, '*');
      window.close();
    }
  </script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_request_path_strips_query_and_method() {
        assert_eq!(parse_request_path("GET /oauth-popup?code=123 HTTP/1.1\r\n"), "/oauth-popup");
        assert_eq!(parse_request_path("GET / HTTP/1.1\r\n"), "/");
        assert_eq!(parse_request_path(""), "/");
    }

    #[test]
    fn test_server_serves_fixtures_on_ephemeral_port() {
        let port = start_test_server().expect("test server should bind");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream
            .write_all(b"GET /oauth-popup HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("200 OK"));
        assert!(resp.contains("Simulasi OAuth"));
    }
}
