# JetStream Desktop

A native desktop app for JetStream, built with [Tauri 2](https://tauri.app) and [Svelte 5](https://svelte.dev). It has its own desktop interface and bundles the JetStream server, so you don't need Python, Anaconda or a terminal to use it.

## What's in the app

- **Synced folders:** each folder you add is a card showing:
  - where it goes in the cloud
  - when it last synced, and how much
  - live progress, speed and time left while it runs, and the file being uploaded
  - its schedule and next run, if it syncs automatically

  **Preview** shows what would upload. **Sync now** uploads only what's new.
- **Add folders your way:** **Add folder** opens the native folder picker, or you can drag a folder from File Explorer onto the window. The dialog counts the folder's files, offers quick bucket choices, and lets you choose:
  - which upload tool to use: `gcloud storage rsync`, or the faster-to-re-sync **JetStream engine** (see [`transfer/README.md`](transfer/README.md))
  - whether to include subfolders
  - whether files already in the cloud may be overwritten
  - which files to skip
  - **Sync automatically:** at a set time on chosen days, or every few hours, optionally as a preview only. Scheduled syncs run while JetStream is open (including from the tray). A sync is skipped if the previous one is still running, and a missing folder (e.g. an unplugged drive) shows up as a failed job you can see.
- **Activity:** every upload, preview and cloud operation, updated live, with filters, search and a details panel. From there you can:
  - see which files are uploading right now and which just finished or failed
  - read the job's output
  - cancel a job, run it again, or "upload for real" after a preview
  - show the job's log file
- **Cloud storage:** browse buckets folder by folder, with sizes, last-updated times and storage classes. From any folder you can:
  - total everything in it
  - copy its `gs://` path
  - **Sync to…** another location (gcloud storage rsync)
  - **Move…** a file or folder, safely: each object is copied and verified before the original is deleted

  Each operation starts as a preview.
- **Cloud cleanup:** scan a bucket or folder for junk: unfinished uploads (`.gstmp`), NAS thumbnail folders, `Thumbs.db`, `.DS_Store`, Office lock files and temporary files. You can add your own patterns. The Rust core does the scan (read-only, tens of thousands of objects a second), then shows:
  - totals and a breakdown by kind, which you can click to filter
  - the matching objects, largest first

  **Move to quarantine…** copies each object to a quarantine bucket, verifies it, then deletes the original. It previews first, never touches protected prefixes, and keeps a record in the classic Cloud audit page.
- **Folder insights:** analyze any local folder in seconds, by choosing it or dropping it on the window. It shows:
  - size by kind of file and by file type
  - the biggest subfolders, which you can click to drill in
  - the largest files
  - cleanup ideas: temporary and system files, and large files untouched for a year

  **Sync this folder…** turns the folder into a synced folder.
- **Analytics:**
  - data uploaded, job counts, success rate and average speed
  - a 30-day chart of completed and failed jobs, with a table view
  - top destinations
  - CSV export
- **Settings:**
  - Google Cloud sign-in status, with a **Sign in with Google** button
  - default upload tool
  - light, dark or system theme
  - notifications
- **More tools:** cloud audit history, the catalog tool, Drive upload, and the original pages. These open in a separate window.
- **Desktop behaviour:**
  - notifications when jobs finish
  - closing the window keeps JetStream running in the tray, so uploads continue; **Quit** in the tray menu stops it
  - opening the app again brings the running window to the front
- **Installers:** an `.msi` or `.exe` on Windows, a `.dmg` on macOS, and an `.AppImage` or `.deb` on Linux.
- **Portable Windows build:** `JetStream-<version>-windows-x64-portable.zip`. Unzip it anywhere (including a USB drive) and run `JetStream.exe`; nothing is installed. Job history, logs and the upload manifest stay in the `JetStreamData` folder next to the program. Delete that folder to share history with an installed copy instead. Needs the WebView2 runtime, which comes with Windows 10 and 11.

## Requirements for users

- **Google Cloud SDK (`gcloud`):** needed for signing in and for gcloud uploads. If it's missing, **Settings** says so and links to the installer.
- **macOS:** apps opened from Finder don't see your shell's `PATH`. Install the SDK in a standard location such as `/usr/local/bin` or Homebrew, or start the app from a terminal with `open -a JetStream`.

## How it works

```
JetStream.app / JetStream.exe        Rust shell (src-tauri/src/main.rs)
 ├─ starts binaries/jetstream-server  bundled JetStream server (PyInstaller)
 │         on 127.0.0.1:<random port>, and reports its status to the UI
 ├─ ships  binaries/jetstream-transfer  Rust upload engine (transfer/)
 ├─ Rust core, in-process (transfer/ as a library): Cloud Storage browsing,
 │         folder totals and parallel local folder analysis
 ├─ main window: the desktop UI (Svelte, app/ → dist/). Jobs, presets,
 │         sign-in and analytics come from the server's HTTP API and live
 │         event stream; browsing and analysis call the Rust core directly
 └─ "classic" window: the original pages from the server, opened from More tools
```

- **Data folder:** the server runs from the per-user JetStream data folder, which holds the database and the `logs/` folder. On Windows that is `%LOCALAPPDATA%\JetStream`.
- **Shared history:** the pip-installed `jetstream` uses the same folder, so both share one job history.
- **Shutdown:** the server exits when the desktop app exits. This includes when the app crashes or is killed.

## Building

### Easiest: one command

```powershell
# Windows (PowerShell), from the repository root
.\desktop\scripts\build_windows.ps1                 # installers + portable zip
.\desktop\scripts\build_windows.ps1 -PortableOnly   # portable zip only (faster)
```

```bash
# any OS
python desktop/scripts/build_desktop.py [--portable-only] [--skip-python-deps] [--package-only]
```

This installs the Python requirements and PyInstaller, then builds:
- the bundled server
- the upload engine
- the UI and app

It puts everything in `desktop/release/`:
- `JetStream-<version>-<os>-<arch>-portable.zip`
- the installers (`.msi`, setup `.exe`, `.dmg`, `.AppImage`, `.deb`)

`--package-only` re-zips an existing build. Use a virtual environment if you don't want the Python packages installed globally.

On Windows you also need the Visual Studio C++ Build Tools and the WebView2 runtime (see the Tauri prerequisites link below).

### Step by step

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
- **CI:** the **Build desktop app** GitHub workflow (`.github/workflows/desktop.yml`) builds for Windows, macOS and Linux:
  - It runs on any push that changes `desktop/`, `jetstream/` or `requirements.txt`, when started from the Actions tab, or when a `desktop-v*` tag is pushed.
  - Each run uploads a `jetstream-desktop-<os>` artifact with the installers and the portable zip. Download it from the run's summary page.
  - A tag such as `desktop-v0.1.29` also creates a draft release with the installers and portable zips attached.
  - The **Run workflow** button only appears once the workflow file is on the default branch.
- **Code signing:** Windows SmartScreen and macOS Gatekeeper warn about unsigned apps. Signing needs certificates that are not set up yet. See the [Tauri signing guides](https://tauri.app/distribute/).
