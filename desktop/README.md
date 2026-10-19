# JetStream Desktop

A native desktop app for JetStream, built with [Tauri 2](https://tauri.app). It packages the same JetStream server and web pages that `pip install noaa-jetstream` gives you. You don't need Python, Anaconda or a terminal to use it.

## What it adds over the browser version

- **One installer:** an `.msi` or `.exe` on Windows, a `.dmg` on macOS, and an `.AppImage` or `.deb` on Linux.
- **Folder fields:** a **Browse…** button on local folder fields, and you can drag a folder from File Explorer onto the window.
- **Notifications:** desktop notifications when uploads, previews and cloud operations finish or fail.
- **Tray icon:** closing the window keeps JetStream running in the tray, so uploads continue. Use **Quit** in the tray menu to stop it.
- **Single instance:** opening the app again brings the running window to the front.
- **Built-in upload engine:** the JetStream engine is included as an upload tool you can choose. It re-syncs unchanged folders almost instantly and resumes large files; see [`transfer/README.md`](transfer/README.md). `gcloud storage rsync` stays the default.

## Requirements for users

- **Google Cloud SDK (`gcloud`):** uploads still run through it. If it's missing or you aren't signed in, the badge in the top-right corner of the app says so and offers **Sign in** or a link to install it.
- **macOS:** apps opened from Finder don't see your shell's `PATH`. Install the SDK in a standard location such as `/usr/local/bin` or Homebrew, or start the app from a terminal with `open -a JetStream`.

## How it works

```
JetStream.app / JetStream.exe   (Rust, src-tauri/src/main.rs)
 ├─ starts  binaries/jetstream-server  (PyInstaller build of the Python server)
 │          on 127.0.0.1:<random free port>
 ├─ shows   ui/index.html ("Starting JetStream…") until /api/version answers
 └─ opens   http://127.0.0.1:<port>/ in the window; the pages call the Tauri
            dialog and notification plugins through window.__TAURI__
            (jetstream/static/js/desktop.js and shared.js)
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
npm run build                                 # installers in src-tauri/target/release/bundle/
```

- **Development:** use `npm run dev`. Rebuild the sidecar after changing Python code, because the app runs the bundled server, not your source tree.
- **Icons:** regenerate them from `jetstream/static/icon.png` with `npm run icons`.
- **CI:** the **Build desktop app** GitHub workflow (`.github/workflows/desktop.yml`) builds installers for Windows, macOS and Linux. Start it from the Actions tab, or push a tag such as `desktop-v0.1.29` to also create a draft release.
- **Code signing:** Windows SmartScreen and macOS Gatekeeper warn about unsigned apps. Signing needs certificates that are not set up yet. See the [Tauri signing guides](https://tauri.app/distribute/).
