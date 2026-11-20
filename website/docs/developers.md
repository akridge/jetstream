# Developers

JetStream is open source: [github.com/akridge/jetstream](https://github.com/akridge/jetstream).

- **The desktop app** (`desktop/`): Tauri 2 shell (`src-tauri/`), Svelte 5 interface (`app/`), and the Rust engine (`transfer/`). [Build instructions](https://github.com/akridge/jetstream/blob/main/desktop/README.md#run-from-source-development).
- **The engine's command line** (`jetstream-transfer`): sync, compare, receipts, catalogs, speed check and more. [Engine README](https://github.com/akridge/jetstream/blob/main/desktop/transfer/README.md).
- **The Python package** (`jetstream/`): the local server and the classic web app.

## Building

```bash
python desktop/scripts/build_desktop.py           # installers + portable zip, for this OS
python desktop/scripts/build_desktop.py --portable-only
```

Every push builds Windows and Linux on GitHub Actions; tags like `desktop-v0.2.1` make a draft release with notes from `desktop/CHANGELOG.md`.

## This site

The site is in `website/` (MkDocs Material) and is published to GitHub Pages on every push that changes it:

```bash
pip install mkdocs-material
mkdocs serve -f website/mkdocs.yml
```
