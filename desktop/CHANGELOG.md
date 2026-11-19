# JetStream Desktop changes

The desktop app has its own version, separate from the `jetstream` Python package.
Each release's notes come from its section here (`## 0.2.0`, ...).

## 0.2.0

The first release of the new desktop app: synced folders, a cloud file manager and a faster
upload engine, in one window.

### Uploads
- **Large files upload in parallel parts** and are joined in the cloud, checked by checksum. Turn it off in Settings if a bucket needs MD5 checksums.
- **Pause and stop take effect at once.** Resuming picks up where it left off, including partly uploaded files.
- **Internet drops are waited out.** Uploads pause while the connection is down and continue when it's back, without failing files.
- **Preview, then upload.** A preview can go straight on to the upload, reusing what it already worked out.
- **Uploads keep file details:** a proper file type (images open in the browser, not as downloads) and the original modified and created dates.
- **Verification receipts** prove that every file reached the cloud intact.
- Speed limit, download to this computer, and drop files onto the window to upload them.

### Folders
- **Folders always show what's new** and can upload new files on their own after a quiet period.
- **Folders are watched:** new files are noticed within seconds.
- **Add several folders at once**, choose subfolders, and upload all of them from one bar.
- **Compare** a folder with its cloud copy, with live progress.
- Scheduled syncs, and a choice to create a folder named after the source in the destination.

### Cloud storage
- A file manager for buckets: rename, move, copy, delete and restore.
- **Thumbnails** for images (optional), with a size-limited cache.
- **Fix file details** adds file types and original dates to files uploaded by older versions, without uploading them again.
- Folder insights and cloud cleanup.

### Windows
- **Portable build:** unzip and run, no install. Settings and history stay next to the app.
- The app checks for new versions and says when one is available (turn this off in Settings).
