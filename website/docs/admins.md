# For IT and admins

## What JetStream is

A desktop app (Windows, Linux) for uploading folders to Google Cloud Storage and managing buckets. It's made of:

- the app window (Tauri / WebView2 on Windows, WebKitGTK on Linux)
- the **JetStream engine**, a Rust program that does uploads, browsing and verification
- a small local **JetStream server** (Python) that keeps job history; it listens on `127.0.0.1` only, on a random port

## Installing

- **Portable zip:** no installation and no administrator rights; everything stays in the folder, including its data (`JetStreamData`).
- **Windows installer:** per-user (`.exe`) or per-machine (`.msi`). Data in `%LOCALAPPDATA%\JetStream`.
- **Linux:** `.deb`, AppImage, or portable zip. Data in `~/.local/share/jetstream`.

Builds aren't code-signed yet, so SmartScreen warns about them.

## Sign-in and credentials

JetStream uses the user's own Google account through the Google Cloud SDK (`gcloud auth login` and application default credentials). It doesn't use or store service-account keys, and it never sees passwords.

## Permissions

What each feature needs in a bucket (for example through the Storage Object Admin role, or a custom role):

| Permission | Used for |
| --- | --- |
| `storage.objects.list` | Browsing, syncing (what's already there), receipts, catalogs |
| `storage.objects.get` | Downloads, previews and thumbnails, verification |
| `storage.objects.create` | Uploads, copies, new folders |
| `storage.objects.delete` | Moves, renames, deletes, replacing changed files |
| `storage.objects.update` | Fix file details |
| `storage.objects.restore` | Restoring deleted files (soft delete) |

Read-only access is fine for browsing, previews and catalogs; JetStream shows a **read-only** badge and disables what isn't allowed.

## Network

JetStream connects to:

- `storage.googleapis.com` and `oauth2.googleapis.com` (Google Cloud Storage and sign-in)
- `api.github.com`, once a day, to check for a newer version (turn off in **Settings → About**)
- `www.fisheries.noaa.gov`, only when a user looks up an InPort record for a catalog

Nothing else. There's no telemetry or analytics.

## Data and privacy

- Files go only to the buckets users choose.
- Job history, logs, the upload manifest (checksums of uploaded files, so they aren't read again) and receipts stay on the computer, in the data folder above.
- Receipts can optionally be copied next to the data in the bucket (`_jetstream/receipts/`).

## The classic web app

The `jetstream` Python package (`pip install`) is the original web app and command line, still available for servers and scripted use. [See the project README](https://github.com/akridge/jetstream#readme).
