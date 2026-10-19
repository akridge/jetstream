"""Build the JetStream server as a single executable for the desktop app.

Tauri expects sidecars at src-tauri/binaries/<name>-<rust target triple>[.exe].
Run from anywhere with the Python environment that has JetStream's requirements
plus PyInstaller installed:

    pip install -r requirements.txt pyinstaller
    python desktop/scripts/build_sidecar.py
"""

import os
import shutil
import subprocess
import sys
from pathlib import Path

DESKTOP = Path(__file__).resolve().parent.parent
REPO = DESKTOP.parent
BUILD = DESKTOP / "build"
BINARIES = DESKTOP / "src-tauri" / "binaries"


def rust_target_triple() -> str:
    override = os.environ.get("TAURI_TARGET_TRIPLE")
    if override:
        return override
    out = subprocess.check_output(["rustc", "-vV"], text=True)
    for line in out.splitlines():
        if line.startswith("host:"):
            return line.split(":", 1)[1].strip()
    raise SystemExit("Could not determine the Rust target triple from `rustc -vV`")


def main() -> int:
    triple = rust_target_triple()
    exe_suffix = ".exe" if sys.platform == "win32" else ""
    sep = ";" if sys.platform == "win32" else ":"

    cmd = [
        sys.executable, "-m", "PyInstaller",
        "--noconfirm",
        "--clean",
        "--onefile",
        "--name", "jetstream-server",
        "--distpath", str(BUILD / "dist"),
        "--workpath", str(BUILD / "work"),
        "--specpath", str(BUILD),
        "--paths", str(REPO),
        "--add-data", f"{REPO / 'jetstream' / 'static'}{sep}jetstream/static",
        "--collect-submodules", "jetstream",
        "--collect-submodules", "uvicorn",
        "--hidden-import", "jetstream.main",
        "--collect-data", "googleapiclient",
        "--copy-metadata", "google-cloud-storage",
        str(DESKTOP / "sidecar" / "jetstream_server.py"),
    ]
    # Console build on purpose: Tauri starts sidecars without a window
    # (CREATE_NO_WINDOW) and reads their output for the startup error page.
    print("Running:", " ".join(cmd))
    subprocess.run(cmd, check=True, cwd=REPO)

    BINARIES.mkdir(parents=True, exist_ok=True)
    built = BUILD / "dist" / f"jetstream-server{exe_suffix}"
    target = BINARIES / f"jetstream-server-{triple}{exe_suffix}"
    shutil.copy2(built, target)
    print(f"Sidecar ready: {target}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
