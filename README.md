# RoleTab MultiRole Browser

Desktop browser untuk **Windows 10 / Windows 11** yang dirancang bagi QA Engineer dan Developer agar dapat membuka beberapa role atau akun sekaligus dalam satu jendela dengan isolasi sesi penuh antar-profil.

RoleTab dibangun di atas **Tauri v2 (Rust)** dan **Microsoft Edge WebView2** menggunakan kontainer `.data_directory()` terpisah per profil serta mode **Ephemeral** terisolasi per tab.

---

## Fitur Utama

### 1. Isolasi Sesi & Manajemen Profil

- **Persistent Role Profile:** Setiap profil menyimpan Cookies, LocalStorage, SessionStorage, IndexedDB, dan Cache di direktori terpisah (`%LOCALAPPDATA%\com.roletab.browser\profiles\<profile-id>`). Dua tab dengan profil yang sama berbagi sesi; profil berbeda terisolasi penuh.
- **Ephemeral Tab:** Setiap tab Ephemeral berjalan dengan mode incognito dan direktori sementara yang bersih per tab dan dihapus otomatis saat tab ditutup.
- **Profile Manager (Side Panel):** Membuat, mengedit nama/warna badge, menetapkan profil default, membersihkan data browsing, memantau ukuran folder data, serta menghapus profil aktif dengan penutupan tab terkait secara aman sebelum penghapusan direktori.
- **Proxy per Profil:** Konfigurasi `Direct`, `HTTP`, atau `SOCKS5` (`host:port`) secara independen untuk setiap profil.
- **Export / Import Profil (`.zip`):** Mengemas metadata dan isi direktori data profil ke arsip `.zip` (`<Downloads>\RoleTab\Exports\`) dengan penutupan tab sementara agar bebas file-lock, serta import aman dengan UUID baru bila terjadi bentrok nama/ID.

### 2. Produktivitas Multi-Role & Navigasi

- **UI Shell Mirip Browser Desktop:** Tab bar horizontal scrollable dengan badge warna role, favicon/loading spinner, status sleeping/pinned/audio, drag-reorder, klik tengah untuk menutup tab, tab overflow menu, serta Unified Address Bar (Omnibox) dengan indikator protokol (`HTTPS`, `Localhost`, `HTTP`, `Peringatan Sertifikat`).
- **Workspace Preset (1-Klik):** Menyimpan kombinasi tab dan role yang sedang terbuka ke `workspaces.json` lalu meluncurkan seluruhnya sekaligus dalam satu klik.
- **Custom Link Context Menu (`ICoreWebView2_11`):** Klik kanan pada tautan di dalam halaman web menyertakan submenu **Buka Link di Profil** untuk membuka tautan langsung ke profil role tertentu atau tab Ephemeral tanpa menghilangkan menu standar browser.
- **Zoom & Find-in-Page per Tab:** Pengaturan tingkat zoom independen per tab (`25%`–`300%`) dan bar pencarian teks dalam halaman (`Ctrl+F`) yang menampilkan jumlah kecocokan tanpa mengekspos API internal ke halaman web.

### 3. Efisiensi Memori, Stabilitas, & Utilitas

- **Smart Tab Sleep (`TrySuspend` / `Resume`):** Tab latar belakang yang idle di-suspend untuk menghemat RAM/CPU berdasarkan durasi idle yang dapat diatur (`5`, `15`, `30` menit, atau `Nonaktif`), dengan pengecualian otomatis untuk tab aktif, tab yang sedang memutar audio (`ICoreWebView2_8`), tab dengan unduhan berjalan, serta tab yang ditandai **Pin / Tetap Aktif**.
- **Task Monitor:** Panel khusus untuk melihat PID proses WebView2, pemakaian RAM nyata (*Working Set* terdeduplikasi per PID), status tab, serta tombol cepat **Sleep Idle**.
- **Crash Recovery (`ProcessFailed`):** Menangkap crash proses renderer maupun browser proses profil secara terpisah tanpa menjatuhkan aplikasi utama dan menyediakan tombol **Pulihkan Tab**.
- **Download Manager per Tab & Profil:** Intersepsi unduhan WebView2 dengan opsi pengelompokan otomatis ke `<Downloads>\RoleTab\<NamaProfil>\`, penamaan unik saat file bentrok, serta tombol **Buka Folder** (`explorer /select`).
- **OAuth / SSO Popup:** Membedakan `target="_blank"` (menjadi tab baru di profil yang sama) dari `window.open()` dengan *window features* (jendela popup sementara yang berbagi sesi agar `window.opener` dan `postMessage` berjalan normal).
- **Single-Instance & Auto-Updater:** Mencegah dua instance membuka folder profil secara bersamaan (`tauri-plugin-single-instance`), menyediakan tombol DevTools (`F12`) di build rilis, serta pemeriksaan update via `tauri-plugin-updater`.

---

## Pintasan Keyboard

| Pintasan                                   | Aksi                                       |
| ------------------------------------------ | ------------------------------------------ |
| `Ctrl + T`                               | Tab baru (sesuai role tab aktif)           |
| `Ctrl + Shift + N`                       | Tab Ephemeral baru                         |
| `Ctrl + W`                               | Tutup tab aktif                            |
| `Ctrl + Tab` / `Ctrl + Shift + Tab`    | Pindah ke tab berikutnya / sebelumnya      |
| `Ctrl + L`                               | Fokus ke Address Bar                       |
| `Ctrl + F`                               | Buka Find-in-Page                          |
| `Ctrl + +` / `Ctrl + -` / `Ctrl + 0` | Perbesar / Perkecil / Reset Zoom tab aktif |
| `F5` / `Ctrl + R`                      | Muat ulang tab aktif                       |
| `F12`                                    | Buka DevTools untuk tab aktif              |

---

## Arsitektur & Keamanan

- **Strict Capability Isolation:** Pada [src-tauri/capabilities/default.json](src-tauri/capabilities/default.json), izin Tauri IPC hanya diberikan kepada view UI internal (`ui-shell`, `profile-panel`, `download-panel`, `task-monitor`). Seluruh child WebView yang memuat situs eksternal berjalan tanpa akses `window.__TAURI__`.
- **Penyimpanan Lokal:** Seluruh profil sesi, preferensi, dan daftar workspace tersimpan secara lokal di direktori aplikasi Windows pengguna (`%LOCALAPPDATA%` dan `%APPDATA%`).
- **Ikonografi & Aset Offline:** Seluruh aset UI Shell (termasuk Font Awesome) dibundel secara lokal di folder [ui/](ui/).

---

## Pengembangan & Build Lokal

### Prasyarat

- Windows 10 atau Windows 11 (x64) dengan Microsoft Edge WebView2 Runtime terpasang
- [Rust](https://www.rust-lang.org/tools/install) (stable toolchain)
- [Node.js](https://nodejs.org/) v22+ (untuk `@tauri-apps/cli`)

### Menjalankan Pengujian & Development

```powershell
# Pasang CLI Tauri lokal
npm ci

# Jalankan seluruh unit & QA suite Rust
cargo test --manifest-path src-tauri/Cargo.toml

# Jalankan aplikasi mode development
npx tauri dev
```

### Build Biner Release & Installer Windows (NSIS)

```powershell
# Build binary release saja
cargo build --manifest-path src-tauri/Cargo.toml --release

# Build installer NSIS (.exe) beserta tanda tangan updater (.sig)
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.tauri\roletab.key"
npx tauri build --bundles nsis
```

Output installer dihasilkan di `src-tauri/target/release/bundle/nsis/`.
