# jetstream-transfer

The Rust core of JetStream, used by the desktop app (as a library) and as a CLI:

- **`sync`:** the upload engine, described below.
- **`ls gs://bucket/folder/`:** one folder level, folders first, with size, updated time and storage class.
- **`du gs://bucket/folder/`:** total objects and bytes under a folder, broken down by storage class.
- **`scan <folder>`:** fast local folder analysis. Subfolders are walked in parallel. It reports:
  - size by kind of file and by file type
  - the biggest subfolders and the largest files
  - junk files
  - archive candidates (large files untouched for a year)
- **`audit gs://bucket/folder/ [--pattern REGEX]…`:** find junk objects in a bucket: unfinished uploads (`.gstmp`), NAS thumbnail folders (`@eaDir`), `Thumbs.db`, `.DS_Store`, `desktop.ini`, Office lock files (`~$…`) and temporary files. Read-only; `--pattern` replaces the default list. Quarantine stays in the Python server, which copies, verifies and only then deletes.

- **`compare <folder> gs://bucket/folder/`:** what's in sync, only local, only in the cloud, or different. `--exclude GLOB` and `--exclude-folder NAME` apply a synced folder's filters, and the upload manifest confirms matches by checksum.
- **File operations:** all server-side, checked, and never overwriting. Add `--plan` to see what would happen without changing anything.
  - `cp SRC... --to gs://b/folder/` and `mv SRC... --to gs://b/folder/`
  - `rename gs://b/path NEW_NAME`
  - `rm SRC...`
  - `mkdir gs://b/parent/ NAME`
  - Folders end with `/`.
- **`bucket gs://b`:** bucket settings (soft delete, hierarchical namespace, Autoclass) and your permissions.
- **`deleted gs://b/folder/` and `restore gs://b NAME#GENERATION...`:** list and restore soft-deleted objects.

`ls`, `du`, `scan`, `audit`, `compare` and the file operations each print one JSON document, the same data the desktop app shows.

## The upload engine

A prototype upload engine for JetStream, written in Rust. It syncs a local folder to `gs://bucket/prefix` with:

- **Incremental re-syncs:** a local manifest (SQLite) records what's already uploaded, so unchanged files are skipped without listing the bucket.
- **Resumable large files:** an interrupted large file continues where it stopped, even after the app restarts.
- **End-to-end checksums:** crc32c is sent with every upload, enforced by Cloud Storage, and checked again on the response.
- **Your own Google sign-in:** OAuth through Application Default Credentials or `gcloud`. No HMAC keys or other shared secrets.
- **Machine-readable progress:** one JSON event per line, ready to feed JetStream's live updates.

In JetStream, it's an **upload tool you can choose**: **Settings → UI Preferences → Default upload tool**, or per job under the Uploads page's **Advanced options**. The choices are:
- **gcloud storage rsync** (the default)
- **JetStream engine (beta)**
- **gsutil rsync** (legacy)

The desktop app bundles the engine beside its server. A pip install shows the option as "not installed" unless `jetstream-transfer` is on `PATH` or `JETSTREAM_TRANSFER_BIN` points to it.

## Why not HMAC keys?

HMAC keys are long-lived secrets tied to a service account.
- **Copies on every desktop:** each laptop would need one, nothing expires it, and a leaked key works from anywhere until someone revokes it.
- **No personal accountability:** audit logs show the service account instead of the person who uploaded.
- **Often blocked:** government orgs commonly forbid HMAC by org policy (`constraints/storage.restrictAuthTypes`).
- **No speed benefit:** HMAC only changes how requests are signed, not how fast bytes move.

## Usage

```bash
cargo build --release
target/release/jetstream-transfer sync D:\Survey\2026 gs://my-bucket/surveys/2026 \
    [--dry-run] [--no-clobber] [--verify] [--exclude '.*\.tmp$'] [--concurrency 32]
```

| Option | Meaning |
|---|---|
| `--dry-run` | Report what would upload; changes nothing (not even the manifest) |
| `--no-clobber` | Never overwrite an existing object (`ifGenerationMatch=0`) |
| `--verify` | Compare every file against a fresh bucket listing instead of trusting the manifest; repairs objects deleted or changed in the cloud |
| `--exclude REGEX` | Skip relative paths (`/` separators) matching the regex; repeatable |
| `--concurrency N` | Files in flight (default 32). `--large-concurrency` (default 4) caps resumable uploads, each holding one chunk in memory |
| `--manifest PATH` | Default: `<JetStream data folder>/transfer-manifest.db` |

**Sign-in,** in order of preference:
- `STORAGE_EMULATOR_HOST`: no auth, for tests.
- `JETSTREAM_ACCESS_TOKEN`: a token supplied by the caller.
- Application Default Credentials of type `authorized_user`: JetStream's **Sign in** button, or `gcloud auth application-default login`. These are refreshed directly with Google, so `gcloud` isn't needed at upload time.
- `gcloud auth print-access-token`.

**Exit codes:** 0 = all synced, 2 = some files failed, 3 = cancelled (Ctrl+C), 1 = error.

**Events** (stdout, one JSON object per line):
- `start` and `plan`: counts of local, unchanged and to-check files.
- `file_uploaded`: includes `resumed_from` when a saved session was continued.
- `file_would_upload`, `file_skipped` and `file_failed`.
- `progress`: once a second.
- `done`: a full summary.

## How a sync works

1. **Walk the folder,** reading only size and mtime.
2. **Skip unchanged files.** If size and mtime match the manifest, the file is unchanged: it isn't read or hashed, and no request is sent.
3. **Hash everything else (crc32c).** A file whose content matches the manifest (only touched or copied) isn't uploaded.
4. **List the bucket only when needed.** On the **first** sync to a destination, or with `--verify`, the prefix is listed once. Files already in the bucket with the same size and crc32c are recorded instead of uploaded, so moving from `gcloud rsync` doesn't re-send data.
5. **Upload in parallel.**
   - Files ≤ 8 MiB go in one multipart request.
   - Larger files use a resumable session whose URI is saved in the manifest. After a restart, the engine asks Cloud Storage how much it already has and sends only the rest.
   - Failed chunks are retried with backoff from the server's confirmed offset.
   - A completed object is only accepted if its size and crc32c match.

## Tests

```bash
cargo test                                    # unit tests + mock-protocol resume tests
go install github.com/fsouza/fake-gcs-server@latest
FAKE_GCS_SERVER=$(go env GOPATH)/bin/fake-gcs-server cargo test   # + emulator end-to-end tests
```

- **Resume tests:** `tests/resume_protocol.rs` runs against a small in-process server that follows Google's documented resumable protocol. fake-gcs-server can't be used for this: it finalizes a truncated object when asked for upload status, which real Cloud Storage doesn't do. A test makes sure the engine rejects such an object and re-uploads.
- **Emulator tests:** `tests/emulator.rs` covers first sync, incremental re-sync, touched-only files, files already in the bucket, no-clobber, dry run and `--verify` repair.

## Benchmarks

### Local emulator (10,000 files, 74 MB)

A fresh emulator was used for each tool, on the same machine.

| | wall time | CPU time | peak memory |
|---|---|---|---|
| gcloud rsync — first upload | 104.2 s | 82.8 s | 90 MB |
| **engine — first upload** | **98.4 s** | **8.6 s** | **41 MB** |
| gcloud rsync — nothing changed | 11.1 s | 10.7 s | 158 MB |
| **engine — nothing changed** | **0.0 s** | **0.0 s** | **11 MB** |
| engine — `--verify` (one bucket listing) | 0.4 s | 0.1 s | 20 MB |

How to read this:
- **First-upload time is limited by the emulator,** which slows down as objects are added, so it says nothing about real network throughput.
- **What carries over to real use:**
  - about 10× less CPU for the same work
  - half the memory
  - re-syncs that do no work at all when nothing changed

  gcloud re-hashes every file and lists the bucket on every run, and that cost grows with folder size.

### Real bucket

Run `bench/bench.sh gs://YOUR-TEST-BUCKET/jetstream-bench`. It compares both tools on:
- 20,000 small files
- 5 × 1 GB files
- an unchanged re-sync

Results go here before deciding to replace `gcloud rsync` in the app.

## Not done yet

- **JetStream's own sign-in (loopback OAuth):** removing the SDK requirement entirely needs an OAuth client ID registered by your Google Cloud org. Until then, sign-in reuses gcloud's ADC.
- **Service-account key files:** these fall back to `gcloud` for now.
- **Bandwidth limit and upload time windows.**
