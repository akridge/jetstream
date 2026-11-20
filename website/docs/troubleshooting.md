# Troubleshooting

??? question "Windows says the download is dangerous, or \"Windows protected your PC\""
    JetStream isn't code-signed yet, so Windows SmartScreen and some browsers warn about it. Choose **Keep** in the browser, then **More info → Run anyway**. Signing is planned.

??? question "Uploads seem slow"
    Run **Speed check…** from the folder's menu. In about half a minute it says whether the disk or network share, the network, or per-file overhead is the limit, and what helps. Also check **Settings → Uploads**: a speed limit may be set, and **Upload large files in parallel parts** should be on.

??? question "The card says \"Waiting for the internet connection\""
    JetStream can't reach Google Cloud. Nothing needs doing: the upload continues by itself when the connection is back, from where it stopped.

??? question "Files are skipped as online-only"
    They're OneDrive "files on demand" that aren't on this computer. Right-click the folder in File Explorer → **Always keep on this device**, then sync again.

??? question "Some files couldn't be read"
    Another program has them open (Excel, an instrument's software). Close it; JetStream tries them again at the end of the sync, and you can **Verify now** afterwards.

??? question "A receipt says files are \"different from the cloud\""
    The file on this computer and its cloud copy have different content: one of them changed after uploading. Open **Compare** to see which, then sync to upload the newer version.

??? question "\"Sign in with Google\" doesn't work"
    JetStream uses the Google Cloud SDK to sign in. Install it from [cloud.google.com/sdk](https://cloud.google.com/sdk/docs/install), restart JetStream, and try again. **Settings** shows what's missing.

??? question "Permission denied (403) on a bucket"
    Your Google account doesn't have the permission needed in that bucket. Cloud storage shows a **read-only** badge when you can only view. See [the permissions list](admins.md#permissions) to send to your bucket's administrator.

??? question "Where are the logs?"
    In the JetStream data folder (`logs/`): `JetStreamData` next to the program for the portable version, `%LOCALAPPDATA%\JetStream` for the installer on Windows, `~/.local/share/jetstream` on Linux. Each job's log is also in **Activity** (click a job).

??? question "Nothing here helps"
    [Open an issue on GitHub](https://github.com/akridge/jetstream/issues) with what you did, what you expected, and the job's log from **Activity**.
