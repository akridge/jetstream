"""Build JetStream Desktop end to end: bundled server, upload engine, app, installers and a
portable package. Works on Windows, macOS and Linux; run it from anywhere.

    python desktop/scripts/build_desktop.py                 # everything
    python desktop/scripts/build_desktop.py --portable-only # skip installers (faster)
    python desktop/scripts/build_desktop.py --package-only  # just re-zip an existing build

Needs Python 3.10+, Rust (stable), Node 18+, and on Windows the WebView2 runtime
(included with Windows 10/11). Output goes to desktop/release/.

Portable package: JetStream.exe (or JetStream on Linux) plus its two helper programs and an
empty JetStreamData folder. Unzip anywhere and run; job history and logs stay in
JetStreamData next to the program. Delete that folder to use the normal per-user data
folder instead (shared with an installed copy and the pip version).
"""

import argparse
import json
import platform
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

DESKTOP = Path(__file__).resolve().parent.parent
REPO = DESKTOP.parent
TAURI = DESKTOP / "src-tauri"
RELEASE_DIR = TAURI / "target" / "release"
OUT = DESKTOP / "release"
IS_WINDOWS = sys.platform == "win32"
EXE = ".exe" if IS_WINDOWS else ""
NPM = "npm.cmd" if IS_WINDOWS else "npm"
NPX = "npx.cmd" if IS_WINDOWS else "npx"

PORTABLE_README = """JetStream (portable)
====================

Run {app} to start JetStream. Nothing is installed.

- Job history, logs and the upload manifest are kept in the JetStreamData folder next to
  the program, so you can carry this folder (for example on a USB drive).
- To share history with an installed copy of JetStream instead, delete JetStreamData.
- Keep {app}, jetstream-server{exe} and jetstream-transfer{exe} together in this folder.
- Requirements: Google Cloud SDK (gcloud) for signing in and gcloud uploads.
  On Windows, the Microsoft Edge WebView2 runtime (built into Windows 10 and 11).
"""


def run(cmd, cwd=REPO):
    print(f"\n==> {' '.join(str(c) for c in cmd)}", flush=True)
    subprocess.run([str(c) for c in cmd], cwd=cwd, check=True)


def version() -> str:
    return json.loads((TAURI / "tauri.conf.json").read_text())["version"]


def arch() -> str:
    machine = platform.machine().lower()
    return {"amd64": "x64", "x86_64": "x64", "arm64": "arm64", "aarch64": "arm64"}.get(machine, machine)


def platform_name() -> str:
    return "windows" if IS_WINDOWS else "macos" if sys.platform == "darwin" else "linux"


def build(args) -> None:
    if not args.skip_python_deps:
        run([sys.executable, "-m", "pip", "install", "--upgrade", "-r", REPO / "requirements.txt", "pyinstaller"])
    run([sys.executable, DESKTOP / "scripts" / "build_sidecar.py"])
    run([sys.executable, DESKTOP / "scripts" / "build_engine.py"])
    run([NPM, "ci"] if (DESKTOP / "package-lock.json").exists() else [NPM, "install"], cwd=DESKTOP)
    tauri = [NPX, "tauri", "build"]
    if args.portable_only:
        tauri.append("--no-bundle")
    run(tauri, cwd=DESKTOP)


def package_portable() -> Path:
    """Zip the built app with its helpers into desktop/release/."""
    app_name = "JetStream" + EXE
    files = {
        app_name: RELEASE_DIR / f"jetstream-desktop{EXE}",
        f"jetstream-server{EXE}": RELEASE_DIR / f"jetstream-server{EXE}",
        f"jetstream-transfer{EXE}": RELEASE_DIR / f"jetstream-transfer{EXE}",
    }
    missing = [str(p) for p in files.values() if not p.exists()]
    if missing:
        raise SystemExit("Build output missing (run without --package-only first):\n  " + "\n  ".join(missing))
    if sys.platform == "darwin":
        print("macOS: the .app inside the .dmg is already portable; skipping the zip.")
        return OUT

    OUT.mkdir(parents=True, exist_ok=True)
    folder = f"JetStream-{version()}-{platform_name()}-{arch()}-portable"
    archive = OUT / f"{folder}.zip"
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as zf:
        for name, source in files.items():
            info = zipfile.ZipInfo.from_file(source, f"{folder}/{name}")
            info.external_attr = 0o755 << 16  # keep Linux executables executable
            with open(source, "rb") as fh:
                zf.writestr(info, fh.read(), compress_type=zipfile.ZIP_DEFLATED)
        for name, text, mode in (("JetStreamData/", "", 0o40755), ("README.txt", PORTABLE_README.format(app=app_name, exe=EXE), 0o644)):
            info = zipfile.ZipInfo(f"{folder}/{name}")
            info.external_attr = mode << 16
            zf.writestr(info, text, compress_type=zipfile.ZIP_DEFLATED)
    print(f"\nPortable package: {archive} ({archive.stat().st_size / 1e6:.0f} MB)")
    return archive


def collect_installers() -> None:
    bundle = RELEASE_DIR / "bundle"
    if not bundle.exists():
        return
    OUT.mkdir(parents=True, exist_ok=True)
    for pattern in ("msi/*.msi", "nsis/*.exe", "dmg/*.dmg", "appimage/*.AppImage", "deb/*.deb"):
        for path in bundle.glob(pattern):
            shutil.copy2(path, OUT / path.name)
            print(f"Installer: {OUT / path.name}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--portable-only", action="store_true", help="build the app but no installers")
    parser.add_argument("--package-only", action="store_true", help="only package an existing build")
    parser.add_argument("--skip-python-deps", action="store_true", help="don't pip install requirements first")
    args = parser.parse_args()

    if not args.package_only:
        build(args)
    package_portable()
    if not args.portable_only:
        collect_installers()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
