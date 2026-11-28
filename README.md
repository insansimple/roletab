<div align="center">

<img src="assets/roletab-logo.svg" alt="RoleTab Logo" width="96" height="96" />

# RoleTab — MultiRole Browser

**Satu jendela browser, banyak identitas role secara bersamaan tanpa bentrok sesi.**

[![Halaman Releases](https://img.shields.io/github/v/release/insansimple/roletab?style=for-the-badge&label=Releases&color=0f766e)](https://github.com/insansimple/roletab/releases)

</div>

---

## Apa Itu RoleTab?

Pernah merasa lelah karena harus terus **login-logout**, membuka banyak jendela *Incognito*, atau memasang tiga browser berbeda hanya untuk menguji akun **Admin**, **Kasir**, dan **User** di aplikasi web yang sama?

**RoleTab** adalah browser desktop ringan untuk **Windows 10 & Windows 11** di mana **setiap tab dapat terikat ke profil peran (role) yang berbeda**. Cookies, LocalStorage, dan sesi login setiap profil terpisah 100%, namun tetap tersusun rapi dalam satu jendela tab seperti Chrome atau Edge.

---

## Mengapa Pakai RoleTab? (Kegunaan Nyata)

| Masalah di Browser Biasa | Solusi di RoleTab |
| --- | --- |
| Login akun baru menimpa sesi akun lama di tab sebelah | Setiap profil punya ruang data terisolasi (`.data_directory()`); buka **Admin** dan **User** berdampingan tanpa saling ganggu |
| Menguji 2–3 akun anonim baru berulang kali merepotkan | Mode **Ephemeral** membuat ruang sesi sekali pakai yang terpisah per tab dan otomatis bersih saat ditutup |
| Harus membuka ulang banyak tab satu per satu tiap pagi | Simpan kombinasi tab & URL favorit ke **Workspace**, lalu buka semua role sekaligus dalam **1 klik** |
| Membuka banyak tab menguras RAM laptop | **Smart Tab Sleep** menidurkan tab latar belakang yang diam otomatis, tetap menjaga tab **Audio** atau yang di-**Pin** |
| Perlu mengetes akses lewat jaringan/VPN berbeda | Setiap profil dapat memakai pengaturan **Proxy** masing-masing (`Direct`, `HTTP`, atau `SOCKS5`) |

### Cocok Untuk:
- **QA Engineer & Tester:** Menguji alur RBAC end-to-end (misalnya pengajuan oleh *Staff* $\rightarrow$ persetujuan *Manager* $\rightarrow$ cek laporan oleh *Admin*) secara langsung dalam hitungan detik.
- **Full-Stack & Frontend Developer:** Menguji `localhost` atau staging lintas akun/role lengkap dengan **DevTools (`F12`)** di tiap tab.
- **Tim Operasional & Multi-Akun:** Mengelola beberapa akun dashboard SaaS, portal internal, atau marketplace dalam satu aplikasi yang rapi tanpa tertukar sesi.

---

## Fitur Unggulan

- **Profil Role Berwarna & Terisolasi:** Bedakan setiap tab secara instan lewat badge warna role (`adm`, `usr`, `eph`, dll.).
- **1-Klik Multi-Role Workspace:** Simpan kumpulan tab role + URL aktif menjadi preset dan luncurkan ulang kapan saja.
- **Klik Kanan Tautan ke Profil Lain:** Klik kanan link di halaman Admin $\rightarrow$ *Buka Link di Profil* $\rightarrow$ langsung buka di tab User atau Ephemeral.
- **Export & Import Profil (`.zip`):** Pindahkan profil beserta sesi login dan pengaturan proxy antar komputer tim dengan aman.
- **Task Monitor & Crash Recovery:** Pantau pemakaian RAM & PID per profil, dan pulihkan tab yang crash tanpa mematikan tab lainnya.
- **Download Manager per Profil:** File unduhan otomatis dapat dirapikan ke folder `Downloads\RoleTab\<NamaProfil>\`.
- **Zoom & Find-in-Page (`Ctrl+F`) per Tab:** Atur zoom independen (`25%`–`300%`) dan cari teks di halaman dengan cepat.
- **OAuth / SSO Popup Friendly & Auto-Updater:** Login Google/Microsoft/Auth0 via `window.open()` berjalan normal sesuai sesi tab pemanggil, lengkap dengan pemeriksaan pembaruan otomatis dari halaman rilis GitHub.

---

## Cara Mulai Cepat

1. Unduh installer terbaru (`RoleTab_..._x64-setup.exe`) dari halaman **[Releases](https://github.com/insansimple/roletab/releases/latest)**.
2. Jalankan installer (tidak memerlukan hak Administrator; terpasang bersih di akun pengguna Windows).
3. Buat profil pertama Anda (misal `Admin` merah dan `User` biru), buka tab berdampingan, dan mulai bekerja tanpa bentrok login.

---

## Pintasan Keyboard

| Pintasan | Fungsi |
| --- | --- |
| `Ctrl + T` | Buka tab baru sesuai role tab aktif |
| `Ctrl + Shift + N` | Buka tab Ephemeral (anonim terisolasi) |
| `Ctrl + W` | Tutup tab aktif |
| `Ctrl + Tab` / `Ctrl + Shift + Tab` | Pindah antar tab |
| `Ctrl + L` | Fokus ke Address Bar |
| `Ctrl + F` | Cari teks di halaman (`Find-in-Page`) |
| `Ctrl + +` / `Ctrl + -` / `Ctrl + 0` | Perbesar / Perkecil / Reset Zoom tab |
| `F5` / `Ctrl + R` | Muat ulang halaman |
| `F12` | Buka Developer Tools tab aktif |

---

## Keamanan & Privasi

- **100% Lokal:** Seluruh sesi login, cookies, preferensi, dan workspace hanya tersimpan di direktori lokal Windows Anda (`%LOCALAPPDATA%` & `%APPDATA%`).
- **Isolasi IPC Ketat:** Situs web yang dibuka di dalam tab berjalan tanpa akses ke API desktop internal (`window.__TAURI__`), sesuai konfigurasi pada [src-tauri/capabilities/default.json](src-tauri/capabilities/default.json).

---

<details>
<summary><strong>Untuk Developer: Cara Build & Uji dari Source</strong></summary>

### Prasyarat
- Windows 10 / 11 (x64) dengan Microsoft Edge WebView2 Runtime
- Rust (stable) & Node.js v22+

```powershell
# 1. Pasang Tauri CLI lokal
npm ci

# 2. Jalankan seluruh unit & QA suite Rust
cargo test --manifest-path src-tauri/Cargo.toml

# 3. Jalankan mode development
npx tauri dev

# 4. Build installer Windows (.exe NSIS)
npx tauri build --bundles nsis
```

</details>
