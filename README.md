# Resale Inventory

A tiny, self-hosted inventory tracker for people who resell on **eBay** and **Facebook Marketplace**.
Install it on your computer, then open it on your phone too. Your data stays on your computer.

- Track name, SKU/bin, cost, price, platform, status (Unlisted / Listed / Sold), listing link, notes and a photo
- **Mark sold** in one tap with the real sale price; profit uses what it actually sold for
- Sort by newest, price, or listed longest; items listed 30+ days get flagged
- **Duplicate** an item to list similar ones fast
- **Open on phone**: scan a QR code, then Add to Home Screen
- Small and light: one program and one database file. No cloud, no account.

## Get started (desktop app, easiest)
1. Download the installer for Windows, Mac or Linux from [Releases](../../releases) and run it.
2. Open **Resale Inventory**. The first time, it asks you to create a password.
3. To use your phone: tap **Open on phone** and scan the QR code (phone on the same Wi-Fi).
   If Windows asks whether to allow the app on the network, allow **Private networks**.

The app keeps your inventory on this computer. Back it up by copying `inventory.db` from
`%APPDATA%\dev.resaleinventory.app` (Windows) or `~/Library/Application Support/dev.resaleinventory.app` (Mac).

Already run Resale Inventory on a home server? Use **Server → Another server…** in the app to connect to it,
and **Server → This computer** to switch back.

## Run on a server (no app)
Download the server program for your system from [Releases](../../releases) and run it. Then open http://localhost:8080
and create your password.

**Docker**
```bash
docker build -t resale-inventory .
docker run -d --name inventory -p 8080:8080 -v inventory-data:/data resale-inventory
```

**Linux (systemd):** see [`deploy/resale-inventory.service`](deploy/resale-inventory.service).

To use it away from home, put it behind HTTPS (e.g. Cloudflare Tunnel or Tailscale), and
**create your password before exposing it**: until a password exists, whoever opens it first gets to set one.

## Settings (server program)
| Variable | Default | What it does |
|---|---|---|
| `RI_ADDR` | `0.0.0.0:8080` | Address and port to listen on |
| `RI_DB` | `inventory.db` | Where the database file lives |
| `RI_PASSWORD` | (none) | Optional fixed password instead of creating one in the browser |

## Build from source
Needs Rust 1.98+.
- Server: `cargo build --release` → `target/release/resale-inventory`
- Desktop app ([Tauri](https://tauri.app), in `app/`): `cargo install tauri-cli --version "^2"`, then `cargo tauri build` inside `app/`

## License
MIT
