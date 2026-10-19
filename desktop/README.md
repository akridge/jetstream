# JetStream Desktop

A native desktop app for JetStream, built with [Tauri 2](https://tauri.app) and [Svelte 5](https://svelte.dev). It has its own desktop interface and bundles the JetStream server, so you don't need Python, Anaconda or a terminal to use it.

## What's in the app

- **Synced folders:** each folder you add is a card showing:
  - where it goes in the cloud
  - when it last synced, and how much
  - live progress, speed and time left while it runs

  **Preview** shows what would upload. **Sync now** uploads only what's new.
- **Add folders your way:** **Add folder** opens the native folder picker, or you can drag a folder from File Explorer onto the window. The dialog counts the folder's files, offers quick bucket choices, and lets you choose:
  - which upload tool to use: `gcloud storage rsync`, or the faster-to-re-sync **JetStream engine** (see [`transfer/README.md`](transfer/README.md))
  - whether to include subfolders
  - whether files already in the cloud may be overwritten
  - which files to skip
- **Activity:** every upload, preview and cloud operation, updated live, with filters, search and a details panel. From there you can:
  - read the job's output
  - cancel a job, run it again, or "upload for real" after a preview
  - show the job's log file
- **Settings:**
  - Google Cloud sign-in status, with a **Sign in with Google** button
  - default upload tool
  - light, dark or system theme
  - notifications
- **More tools:** cloud operations, cloud and local audits, the catalog tool, analytics and Drive upload. These open the original pages in a separate window while they're rebuilt for the new interface.
- **Desktop behaviour:**
  - notifications when jobs finish
  - closing the window keeps JetStream running in the tray, so uploads continue; **Quit** in the tray menu stops it
  - opening the app again brings the running window to the front
- **Installers:** an `.msi` or `.exe` on Windows, a `.dmg` on macOS, and an `.AppImage` or `.deb` on Linux.

## Requirements for users

- **Google Cloud SDK (`gcloud`):** needed for signing in and for gcloud uploads. If it's missing, **Settings** says so and links to the installer.
- **macOS:** apps opened from Finder don't see your shell's `PATH`. Install the SDK in a standard location such as `/usr/local/bin` or Homebrew, or start the app from a terminal with `open -a JetStream`.

## How it works

```
JetStream.app / JetStream.exe        Rust shell (src-tauri/src/main.rs)
 ├─ starts binaries/jetstream-server  bundled JetStream server (PyInstaller)
 │         on 127.0.0.1:<random port>, and reports its status to the UI
 ├─ ships  binaries/jetstream-transfer  Rust upload engine (transfer/)
 ├─ main window: the desktop UI (Svelte, app/ → dist/), which calls the
 │         server's HTTP API and live event stream, and uses Tauri for the
 │         folder picker, drag-and-drop, notifications and opening files
 └─ "classic" window: the original pages from the server, opened from More tools
```

- **Data folder:** the server runs from the per-user JetStream data folder, which holds the database and the `logs/` folder. On Windows that is `%LOCALAPPDATA%\JetStream`.
- **Shared history:** the pip-installed `jetstream` uses the same folder, so both share one job history.
- **Shutdown:** the server exits when the desktop app exits. This includes when the app crashes or is killed.

## Building

You need Python 3.10+, Rust (stable), Node 18+, and the [Tauri system prerequisites](https://tauri.app/start/prerequisites/) for your OS.

```bash
# from the repository root
pip install -r requirements.txt pyinstaller
python desktop/scripts/build_sidecar.py      # -> desktop/src-tauri/binaries/jetstream-server-<target>
python desktop/scripts/build_engine.py       # -> desktop/src-tauri/binaries/jetstream-transfer-<target>

cd desktop
npm install
npm run build                                 # builds the UI, then installers in src-tauri/target/release/bundle/
```

- **Development:** `npm run dev` runs the app with live-reloading UI. Rebuild the sidecar after changing Python code, because the app runs the bundled server, not your source tree.
- **UI only, in a browser:** start `jetstream` with `CORS_ALLOW_ORIGINS=http://localhost:5173`, run `npm run dev:ui`, and open `http://localhost:5173/?api=http://127.0.0.1:8000`. Desktop-only features (folder picker, drag-and-drop) fall back to simple prompts.
- **Icons:** regenerate them from `jetstream/static/icon.png` with `npm run icons`.
- **CI:** the **Build desktop app** GitHub workflow (`.github/workflows/desktop.yml`) builds installers for Windows, macOS and Linux. Start it from the Actions tab, or push a tag such as `desktop-v0.1.29` to also create a draft release.
- **Code signing:** Windows SmartScreen and macOS Gatekeeper warn about unsigned apps. Signing needs certificates that are not set up yet. See the [Tauri signing guides](https://tauri.app/distribute/).
