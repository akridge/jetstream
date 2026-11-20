# JetStream Desktop

A native desktop app for JetStream, built with [Tauri 2](https://tauri.app) and [Svelte 5](https://svelte.dev). It has its own desktop interface and bundles the JetStream server, so you don't need Python, Anaconda or a terminal to use it.

**User guide, downloads and screenshots: [akridge.github.io/jetstream](https://akridge.github.io/jetstream/)** (the site's source is in [`website/`](../website)).

## Ways to run it

| You want to… | Use | Needs |
|---|---|---|
| **Use JetStream** | A built app: installer or portable zip ([Get the app](#get-the-app)) | Google Cloud SDK for signing in. Nothing else |
| **Work on the code** | Development mode from this repository ([Run from source](#run-from-source-development)) | Python, Rust, Node |
| **Make your own installer or portable zip** | The build scripts ([Build installers and the portable zip](#build-installers-and-the-portable-zip)) | Python, Rust, Node |

The web version (`pip install` and `jetstream` in a browser) is described in the [main README](../README.md). It shares the same job history with the desktop app.

## Get the app

**From GitHub Actions** (every push that changes the app builds it):
1. Open the repository's **Actions** tab, then the **Build desktop app** workflow.
2. Open the latest green run and scroll to **Artifacts**.
3. Download the artifact for your system:
   - `jetstream-desktop-windows-latest`: the portable zip, an `.msi` installer and a setup `.exe`
   - `jetstream-desktop-ubuntu-22.04`: a portable zip, an `.AppImage` and a `.deb` (Linux)
   - `jetstream-desktop-macos-latest`: a `.dmg` (only when the macOS workflow is run by hand)
4. Unzip the artifact you downloaded (GitHub wraps every artifact in a zip).

**From a release:** tagged versions (`desktop-v*`) have the same files attached to a GitHub release, with notes from [CHANGELOG.md](CHANGELOG.md).

**Updates:** once a day the app asks GitHub for the latest desktop release and, if it's newer, says so at the bottom of the sidebar and in **Settings → About** with a link to download it. Nothing is downloaded or installed on its own; turn the check off in **Settings → About**. To move a portable copy to a new version, unzip the new one and move the `JetStreamData` folder into it.

**Installer or portable?**
- **Installer** (`.msi` / setup `.exe`): adds JetStream to the Start menu and keeps data in your user profile (`%LOCALAPPDATA%\JetStream`).
- **Portable** (`JetStream-<version>-windows-x64-portable.zip`):
  - Unzip it anywhere, including a USB drive, and run `JetStream.exe`. Nothing is installed.
  - Job history, logs and the upload manifest stay in the `JetStreamData` folder next to the program.
  - Delete `JetStreamData` to share history with an installed copy instead.

**On Linux:**
- **`.deb`** (Debian, Ubuntu 22.04 and newer): `sudo apt install ./JetStream_<version>_amd64.deb` installs it with what it needs (WebKitGTK 4.1).
- **`.AppImage`** (most distributions): `chmod +x JetStream_<version>_amd64.AppImage`, then run it.
- **Portable zip** (`JetStream-<version>-linux-x64-portable.zip`): unzip and run `./JetStream`. It needs WebKitGTK 4.1 installed (`libwebkit2gtk-4.1-0` on Ubuntu/Debian).
- The tray icon needs a desktop with AppIndicator support (GNOME needs the AppIndicator extension).

**First run:**
- Windows warns about unsigned apps. Choose **More info → Run anyway**.
- Open **Settings → Sign in with Google**. This needs the [Google Cloud SDK](https://cloud.google.com/sdk/docs/install).
- Windows 10 and 11 include the WebView2 runtime the app needs.

## What's in the app

- **Synced folders:** each folder you add is a card showing:
  - where it goes in the cloud
  - when it last synced, and how much
  - live progress, speed and time left while it runs, and the file being uploaded
  - **Pause** and **Resume** for uploads using the JetStream engine (nothing is lost; gcloud uploads can only be cancelled)
  - what the engine is doing before it uploads ("Checking what's already in the cloud", "Reading files to check: 1,204 of 8,330"), and files being retried
  - progress that survives **Cancel** and **Sync now**: what's already in the cloud counts straight away instead of starting again from 0%
  - its schedule and next run, if it syncs automatically

  **Always up to date:** each card says **Up to date ✓** or **37 new or changed files · 1.2 GB** with an **Upload 37 files** button. JetStream checks every folder quietly in the background (names and sizes against the cloud; nothing is read or uploaded) when it opens, after each sync, and every 15 minutes (per folder: every 5 minutes to every 4 hours, or only when asked; big folders such as network shares are checked less often). **Check for changes now** is in the card's menu. Folders on this computer are also **watched**: new or changed files are noticed within seconds (the card says "watching for new files"); folders that can't be watched, such as some network shares, rely on the timed checks.

  **Upload new files automatically** (per folder): when a check finds new or changed files, JetStream looks again two minutes later and uploads once nothing has changed in between, so files still being copied in aren't uploaded half-done. The card shows when it will start.

  **Preview** shows what would upload. **Sync now** uploads only what's new (it waits its turn if other folders are uploading). While a preview runs, **Upload when done** starts the upload as soon as it finishes. Adding or editing a folder offers **Save & upload** to skip the preview.
- **Several folders or a network share:** **Add several…** lists the folders inside a folder or share (`\\server\share\projects` works) with a tick box each (filter, select all). The ticked ones become separate synced folders under one destination (`gs://bucket/path/<folder name>/`), with the same upload tool, auto-upload and receipt choices; folders already synced are marked. Dropping several folders on the window opens the same dialog, and **Add folders from somewhere else…** adds folders from anywhere. **Add & upload** uploads them one after another.

  With several folders, a bar at the top says **3 folders have new files · 10 files · 879 KB · Upload all**. **Select** puts a tick box on each card for **Upload**, **Preview**, **Check for changes**, **Tags…**, **Auto-upload on/off**, **Hide** or **Remove** on the chosen ones. Finished uploads close together are announced in one message, and receipts that verify everything don't pop up a message at all (the card shows them).
- **Finding your way around many folders:**
  - **At a glance:** a summary shows how much is in the cloud and up to date, how many folders are verified, and whether anything needs you (**1 folder needs you · Show them**).
  - **Search** (press `/`) by name, path, bucket or tag.
  - **Filters** with counts: **All · Needs attention · New files · Syncing · Up to date · Hidden**.
  - **Tags:** tag folders by project, cruise or season (**Tags…** in a folder's menu, or for several at once), then click a tag to show just those.
  - **Hide** finished projects: **Hide from the list** keeps a folder out of the way (with Undo). It still syncs on its schedule and auto-uploads; find it under **Hidden**.
  - **Sort** by what needs you first (the default: failures and unverified folders at the top, then running, then new files, then up to date), name, last synced or most new files.
  - **Cards or a compact list:** one row per folder with a status dot, what's new, tags and last sync. Click a row to expand it into its card.
  - **Keyboard:** `/` search, `↑` `↓` move, `Enter` expand (list) or compare (cards), `S` sync, `P` preview, `V` verify, `E` edit, `T` tags, `Esc` clear, `?` show these.
  - **Upload complete:** when a sync finishes, its card says so for a moment ("Uploaded 1,240 files · 8.4 GB in 12m · 11.9 MB/s"); the desktop notification says the same.
- **Getting started:** a new install shows three steps (sign in, add a folder, preview then upload), ticked off as they happen, then "You're all set". Milestones (1,000 files, 1 TB, …) get a small message. After an update, **What's new** shows the release notes once (also in Settings → About).
- **Activity** is grouped by day, each with what it uploaded ("Today · 3 syncs · 1,240 files uploaded · 8.4 GB").
- **Starred locations:** the star in Cloud storage's toolbar keeps a bucket or folder in the sidebar under Cloud storage.
- **Notifications** (Settings → Appearance & notifications): for every finished job, only for problems (a failed sync, files that couldn't be verified), or off; with optional quiet hours.
- **Search or do anything (Ctrl+K, ⌘K on a Mac, or Search at the top of the sidebar):** type to jump to any view or run an action on any folder: "sync kona", "receipt pago", "open hilo in cloud storage", "catalogs", "speed check", "gs://my-bucket/path".
- **Add folders your way:** **Add folder** opens the native folder picker, or you can drag a folder from File Explorer onto the window. The dialog counts the folder's files, offers quick bucket choices, and lets you choose:
  - which upload tool to use: `gcloud storage rsync`, or the faster-to-re-sync **JetStream engine** (see [`transfer/README.md`](transfer/README.md))
  - **Create a folder in the destination:** upload the folder itself rather than just its contents. `C:\folder1` to `gs://bucket/folder` then lands in `gs://bucket/folder/folder1/`. New folders remember your last choice.
  - whether to include subfolders
  - **Upload each subfolder as its own job:** for big folders. Each subfolder finishes on its own, a problem only redoes that subfolder, and files directly in the folder get a job too. The card shows "Folder 5 of 12", **Cancel all** stops the lot and **Retry failed folders** redoes only what failed. In Activity the subfolders sit under one row you can expand.
  - whether files already in the cloud may be overwritten
  - which files to skip
  - **Sync automatically:** at a set time on chosen days, or every few hours, optionally as a preview only. Scheduled syncs run while JetStream is open (including from the tray). A sync is skipped if the previous one is still running, and a missing folder (e.g. an unplugged drive) shows up as a failed job you can see.
- **Activity:** every upload, preview and cloud operation, updated live, with filters, search and a details panel. From there you can:
  - see which files are uploading right now and which just finished or failed
  - read the job's output
  - cancel a job, run it again, or "upload for real" after a preview
  - show the job's log file
- **Cloud storage:** a file manager for buckets. Browse folder by folder, with sizes, the original files' modified dates and storage classes.
  - **File details:** each upload records its file type (from the extension, like gcloud) and the original file's modified and created dates (custom metadata `goog-reserved-file-mtime`, as gcloud uses, and `jetstream-file-created`).
    - The **Modified** column shows the original modified date; hover for created, modified and uploaded. Files uploaded without one show their upload time, marked "(uploaded)". **Size** on a folder also shows the range of original dates inside it.
    - Why the Cloud console looks different: its **Created** and **Last modified** are always when the file reached the bucket (or its details last changed). Cloud Storage sets those, and no uploader can change them. The original dates are under each object's **Custom metadata** in the console. Folders have no dates in normal buckets because they aren't real objects.
    - **Fix file details in the cloud…** (folder card menu, or right-click a folder in Cloud storage) adds what older uploads are missing: the file type, and the original dates from the synced folder on this computer for files that are the same as their cloud copy (checked by checksum). It all happens in one window: a check with live progress (and **Stop**), then what will change, with before and after for each file, then the update with progress, then a summary (and any files that failed, with **Check again**). Nothing is re-uploaded, and files changed since the check are left alone. For a cloud folder that isn't synced, **Fix file types…** sets the types only. Needs permission to update objects (`storage.objects.update`).
  - **Thumbnails** (off until you turn it on with the toolbar button; remembered): images show as a grid of thumbnails. Selection, right-click, rename and the rest work the same. Designed not to waste downloads or disk:
    - only the tiles on screen load, at most 4 at a time, and leaving the folder stops the rest
    - camera JPEGs use the preview embedded in the file, so only their first 128 KB is downloaded
    - other images download only if they're under the size cap (20 MB by default); bigger ones show "Too large · click to preview"
    - Nearline, Coldline and Archive images wait for a click (retrieving them is charged)
    - thumbnails (about 6 KB each) go in a cache with a size limit (250 MB by default; the oldest go first), in the system's cache folder, or in `JetStreamData` for the portable copy. A replaced file gets a new thumbnail. **Settings → Cloud storage thumbnails** shows the cache size, clears it, and sets the limits.
  - **Preview pane** (on by default; close it with its ×, bring it back with **Preview** in the toolbar; drag its edge to resize): selecting a file shows
    - **images** larger (1024 px, cached like thumbnails; same download limit and cold-storage click)
    - **CSV and TSV files** as a table of the first 200 rows, and **text, JSON, XML, logs and Markdown** as text (JSON pretty-printed). Only the first 64 KB is read, however big the file.
    - **every detail Cloud Storage keeps:** type, size, storage class, the original created and modified dates, upload time, CRC32C and MD5 (files uploaded in parallel parts have no MD5, and it says so), generation and any custom metadata, with Copy path, Download and Console buttons.

    Selecting a folder shows its path and a **Size** button; several items show their count and total size. Content loads only for the file you settle on (a quarter-second pause), so arrowing through a folder doesn't download every file.
  - **Select like Explorer:** click; Ctrl/Shift+click for several; arrow keys; right-click for a menu.
  - **Rename** (F2), **New folder**, **Move or copy to…**, and **Delete** (Del). Each shows what it involves before running:
    - how many files and how much data
    - name clashes (existing files are never overwritten)
    - objects under a hold
    - **early-deletion fees** for Nearline, Coldline or Archive files younger than their minimum storage period
  - **How files move:** copied inside Google Cloud (nothing downloads), each copy checked by checksum and size, and only then the original removed.
    - Metadata and storage class are kept.
    - An interrupted move can simply be run again.
    - Renaming never merges into an existing name.
  - **Undo:** for renames and moves (up to 1,000 files), and for deletes in buckets with soft delete.
  - **Recently deleted:** in buckets with soft delete, lists deleted files with **Restore**. A restore never replaces a file that exists now.
  - **Real folders:** buckets with hierarchical namespace rename folders in one step, however many files they hold.
  - **Permissions:** actions you don't have permission for are greyed out. Read-only buckets are labelled.
  - **Download…** files or folders to this computer. Each file is checked against the cloud checksum before it's saved, and files you already have are never replaced.
  - **Upload by dropping** files or folders onto the window while a cloud folder is open (or right-click → **Upload a folder here…**). Folders upload as jobs you can follow in Activity; nothing in the cloud is overwritten.
  - **More actions:** total a folder's size, copy `gs://` paths (Ctrl+C), copy a public link, open in the Cloud Console, **Sync to…** another location (gcloud storage rsync).
  - **Synced folders:** inside a synced folder's cloud copy, a banner shows which folder on this computer it comes from.
- **Verification receipts:** after each sync (on by default; per folder), JetStream checks every file against the cloud by checksum and saves a receipt for data management plans and archiving:
  - a CSV (summary at the top, one row per file: path, status, size, crc32c, MD5, cloud version, time), the same as JSON, and a printable page (open it to print or save as PDF)
  - who checked it, when, and whether the folder is **complete** (every file verified) or which files are missing or different
  - kept in the JetStream data folder (`receipts/<folder>/`; `JetStreamData` in portable mode), optionally with a copy next to the data in `_jetstream/receipts/`
  - it runs quietly: the folder card shows **Verifying… 8,200 of 12,000 files**, then **Verified · 12,000 files**. You're only told when something isn't verified (the card turns amber: **3 files not verified · Review**). **Verify now** in the folder menu runs one any time.
  - clicking it opens the **receipt**: a seal (all verified, or how many aren't), where from and to, when and by whom, and how it was checked. Files that aren't verified are grouped with what to do: **Not in the cloud** (**Upload them**), **Different from the cloud** (**Compare**), **Couldn't be read**, **Online-only (OneDrive)**. Earlier receipts are listed alongside; **Verify again**, **Printable receipt**, **CSV** and **Copy summary** are at the bottom. The Compare screen's receipt list opens them too.
  - only files JetStream hasn't read before are read, so repeat receipts are quick
- **Compare with cloud:** for any synced folder, see exactly what is:
  - **in sync:** on both, same size, and confirmed by checksum when the JetStream engine uploaded it
  - **only on this computer:** uploads on the next sync
  - **only in the cloud:** e.g. added by someone else, or deleted locally
  - **different:** changed on one side

  There's a per-folder breakdown and a button for each file to show it on this computer or in the cloud. The folder's skip filters are applied. **Upload N files** starts the sync right there: its progress shows at the top, and the comparison refreshes when it finishes.

  Compare is the quick look (sizes only). **Preview** goes further and confirms same-size files by checksum, but only reads those: files not in the cloud, or with another size, are listed without being read. What a preview confirms is remembered, so the upload after it doesn't read those files again.
- **Catalogs:** build a catalog of the images (or any files) in a bucket folder, grouped by site, without the 50,000-file limit of the classic tool. Open it from the sidebar, or right-click a folder in Cloud storage → **Make a catalog…**.
  - **Files:** **Images** by default (JPEG, PNG, TIFF, BMP, WebP). Add Camera RAW, Video, Science data (NetCDF, HDF5, Zarr, Parquet, GRIB, …), Tables and text, Documents or Audio and sonar, choose **All files**, or list your own extensions. **More filters:** file name (a word or a pattern like `*_00?_*.jpg`), path, folders to skip (e.g. `thumbs`), folder depth and a file limit.
  - **Groups:** by the start of the file name, by folder level, by a regular expression, or by your own group list. Site instances can be groups with at least N files, groups that are a folder, groups with a file matching a pattern, or groups you tick. The bucket is listed once, so changing any of this updates the result straight away. Each group shows its file count, size and sample thumbnails; click one to browse its files with the preview pane, and untick a group to leave it out.
  - **Export:**
    - **Save OASIS JSON** writes exactly the file the classic catalog tool writes for the same options (`instances`, `instances_grouped`, `site_instances`), byte for byte.
    - **Detailed CSV / JSON** list every file with its group, size, type, original created and modified dates, upload time, CRC32C, MD5, generation and storage class.
    - **Save to my catalogs** keeps the catalog with JetStream (shared with the classic page). Opening it later builds it again from the bucket with the same options.
  - **Dataset information (optional):** enter a NOAA InPort catalog item number (JetStream reads its ISO record from `fisheries.noaa.gov/inport/item/<number>/iso19115`), any ISO 19115 record address, or an ISO XML file. Its title, abstract, time period, area, contacts, keywords, use constraints and DOI go into detailed exports and saved catalogs, and can be saved next to the OASIS JSON as `<name>.dataset.json`. The OASIS JSON itself never changes.
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
  - **speed limit** for uploads and downloads (e.g. 20 Mbit/s on a shared ship or field-station connection). It changes running engine uploads too.
  - **uploads at once** (1–4), e.g. how many subfolders of a split upload run together
  - **Upload large files in parallel parts** (on by default; JetStream engine): files over 256 MB go up as up to 32 parts at once and are joined in the bucket. This is often several times faster on long-distance connections. The joined file is verified by CRC32C but has no MD5 checksum, so turn it off if your data needs MD5. It's skipped automatically for buckets with a retention policy or a Nearline, Coldline or Archive storage class. The job details show "in 16 parallel parts".
  - light, dark or system theme
  - notifications
- **More tools:** cloud audit history, the classic catalog tool, Drive upload, and the original pages. These open in a separate window.
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
 ├─ Rust core, in-process (transfer/ as a library): Cloud Storage browsing and
 │         file operations, folder totals, bucket cleanup scans, local-vs-cloud
 │         comparison and parallel local folder analysis
 ├─ main window: the desktop UI (Svelte, app/ → dist/). Jobs, presets,
 │         sign-in and analytics come from the server's HTTP API and live
 │         event stream; browsing and analysis call the Rust core directly
 └─ "classic" window: the original pages from the server, opened from More tools
```

- **Data folder:** the server runs from the per-user JetStream data folder, which holds the database and the `logs/` folder. On Windows that is `%LOCALAPPDATA%\JetStream`.
- **Shared history:** the pip-installed `jetstream` uses the same folder, so both share one job history.
- **Shutdown:** the server exits when the desktop app exits. This includes when the app crashes or is killed.

## If uploads seem stuck or slow

- **Not sure why it's slow? Run a speed check:** **Speed check…** in a synced folder's menu (or **Settings → Uploads**). In about half a minute it measures:
  - how fast the folder lists and reads (big files and small ones)
  - upload speed over one connection and over 8 at once
  - the time each request to Cloud Storage takes

  Then it says which is the limit: the disk or share, the network, or per-file overhead. Nothing is stored in the bucket (the test uploads are never finished), and the speed limit isn't applied while it runs. **Copy results** gives a text summary to share.
- **Network shares:** folders are now read many at a time (16 at once for `\\server\share` paths, 8 otherwise), instead of one after another. On a share each folder read waits on the file server, so this cuts the time spent listing big shares. Later syncs only need that listing: what's already uploaded isn't read again.

- **OneDrive folders (e.g. Desktop or Documents):** online-only files aren't on the computer, so they're skipped with a note in the job output. Right-click the folder and choose **Always keep on this device**, then sync again.
- **Files open in another program** are tried again at the end of the sync; close the program if they still fail.
- **A dropped internet connection** (JetStream engine): the card says **Waiting for the internet connection**, and everything carries on by itself when it's back, however long it was down, without failing files. Large files continue from where Google has them. A connection that silently stops moving is noticed within a minute.
- **Pause** (JetStream engine) stops sending within a second, even in the middle of a large file; **Resume** carries on from there. **Cancel** stops just as quickly and keeps what's done, so the next run continues.
- **A big folder:** turn on **Upload each subfolder as its own job**. Starting a sync or preview answers at once whatever the folder's size: the card says **Starting…**, then **Counting files… N found so far** (gcloud uploads count first so progress has a total; the JetStream engine counts as it goes). Clicking again while it runs doesn't start a second copy; you're told it's already running.
- **Moving from gcloud to the JetStream engine:** the first engine sync recognises files gcloud uploaded by size and date, without reading them again.

## Run from source (development)

You need:
- Python 3.10+
- Rust (stable)
- Node 18+
- the [Tauri system prerequisites](https://tauri.app/start/prerequisites/) for your OS (on Windows: Visual Studio C++ Build Tools and WebView2)

Once, from the repository root, build the two helper programs the app starts:

```bash
pip install -r requirements.txt pyinstaller
python desktop/scripts/build_sidecar.py   # bundled server  -> desktop/src-tauri/binaries/
                                          # (--portable also builds it as a folder for the portable zip)
python desktop/scripts/build_engine.py    # upload engine   -> desktop/src-tauri/binaries/
cd desktop && npm install
```

Then run the app:

```bash
cd desktop
npm run dev          # opens the app; UI changes reload instantly, Rust changes rebuild and restart
```

**What reloads, and what doesn't:**
- **UI** (`app/`): reloads live.
- **Rust shell and core** (`src-tauri/`, `transfer/`): rebuilt automatically by `npm run dev`.
- **Python server** (`jetstream/`): the app runs the *bundled* server, so re-run `build_sidecar.py` after changing Python code.
- **Upload engine** (`transfer/` used for uploads): re-run `build_engine.py` after changing it.

**UI only, in a browser:** quick for UI work without Rust.
1. Start the server: `CORS_ALLOW_ORIGINS=http://localhost:5173 jetstream`.
2. In `desktop/`, run `npm run dev:ui`.
3. Open `http://localhost:5173/?api=http://127.0.0.1:8000`.

Features that run in the Rust core (Cloud storage, Compare, Cloud cleanup, Folder insights) need the desktop app.

**Command line:** everything the Rust core does is also a CLI, e.g. `cargo run --release -- ls gs://bucket/folder/` in `desktop/transfer`. See [`transfer/README.md`](transfer/README.md).

**Tests:**
- **Python:** `pytest` from the repository root.
- **Rust:** `cargo test` in `desktop/transfer`. Set `FAKE_GCS_SERVER` to a [fake-gcs-server](https://github.com/fsouza/fake-gcs-server) binary to include the emulator tests.

## Build installers and the portable zip

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
- `JetStream-<version>-<os>-<arch>-portable.zip` (CI builds add the build number: `JetStream-0.2.0-build18-windows-x64-portable.zip`). Inside, the server is a folder (`server/`) rather than a self-extracting program, which browsers and virus scanners are much less suspicious of. The programs carry version details and the JetStream icon.
- the installers (`.msi`, setup `.exe`, `.dmg`, `.AppImage`, `.deb`)

`--package-only` re-zips an existing build. Use a virtual environment if you don't want the Python packages installed globally.

On Windows you also need the Visual Studio C++ Build Tools and the WebView2 runtime (see the Tauri prerequisites link below).

### Step by step

After the one-time setup in [Run from source](#run-from-source-development):

```bash
cd desktop
npm run build        # builds the UI, then installers in src-tauri/target/release/bundle/
python scripts/build_desktop.py --package-only   # adds the portable zip; copies everything to release/
```

- **Icons:** regenerate them from `jetstream/static/icon.png` with `npm run icons`.
- **Releasing:** add a `## <version>` section to `CHANGELOG.md`, set the version with `set_version.py`, push, then tag: `git tag desktop-v0.2.1 && git push origin desktop-v0.2.1`. When the build finishes, publish the draft release.
- **Versions:** the desktop app (with its upload engine) has its own version, separate from the `jetstream` Python package. It lives in `src-tauri/tauri.conf.json`. Change it everywhere with `python desktop/scripts/set_version.py 0.2.1`; `--check` confirms the files agree. Every CI build also carries its build number and commit. They show in the sidebar ("v0.2.0 · build 18"), in **Settings → About** (desktop app, upload engine and server versions), in `jetstream-transfer --version`, and in the portable zip's name. Local builds say "local build".
- **CI:** the **Build desktop app (Windows and Linux)** GitHub workflow (`.github/workflows/desktop.yml`) builds both on every push that changes the app:
  - It runs on any push that changes `desktop/`, `jetstream/` or `requirements.txt` (README-only changes are skipped), when started from the Actions tab, or when a `desktop-v*` tag is pushed.
  - Each run uploads `jetstream-desktop-windows-latest` (portable zip, `.msi`, setup `.exe`) and `jetstream-desktop-ubuntu-22.04` (portable zip, `.AppImage`, `.deb`). Download them from the run's summary page. Linux builds on Ubuntu 22.04 so the AppImage runs on older distributions too.
  - A tag such as `desktop-v0.2.0` (which must match the desktop version) also creates a draft release with the installers and portable zip attached. Its description is that version's section of [CHANGELOG.md](CHANGELOG.md) (`scripts/release_notes.py`); the build fails if the section is missing. Publish the draft to make it the "latest" release the app's update check sees.
  - The macOS build is parked in **Build desktop app (macOS)** (`.github/workflows/desktop-mac-linux.yml`), which only runs when started from the Actions tab.
  - The **Run workflow** button only appears once the workflow file is on the default branch.
- **Code signing:** Windows SmartScreen and macOS Gatekeeper warn about unsigned apps. Signing needs certificates that are not set up yet. See the [Tauri signing guides](https://tauri.app/distribute/).
