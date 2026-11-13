"""Build the JetStream server for the desktop app.

Tauri expects sidecars at src-tauri/binaries/<name>-<rust target triple>[.exe] (a single
executable, used by the installers). With --portable it is also built as a folder
(build/portable/jetstream-server/) for the portable zip: a plain program plus its files is
far less likely to be flagged by browsers and virus scanners than a self-extracting one.
On Windows both get version details (JetStream, the desktop version and build) and the icon.

Run from anywhere with the Python environment that has JetStream's requirements
plus PyInstaller installed:

    pip install -r requirements.txt pyinstaller
    python desktop/scripts/build_sidecar.py [--portable]
"""

import argparse
import json
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


def desktop_version() -> str:
    return json.loads((DESKTOP / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))["version"]


def version_text(version: str) -> str:
    build = os.environ.get("JETSTREAM_BUILD", "").strip()
    commit = os.environ.get("JETSTREAM_COMMIT", "").strip()[:7]
    extra = ", ".join(p for p in (f"build {build}" if build else "", commit) if p)
    return f"{version} ({extra})" if extra else f"{version} (local build)"


def windows_version_file() -> Path:
    """PyInstaller version resource: what Explorer and virus scanners see about the program."""
    version = desktop_version()
    build = os.environ.get("JETSTREAM_BUILD", "0").strip() or "0"
    numbers = [int(n) for n in version.split(".")[:3]] + [min(int(build) if build.isdigit() else 0, 65535)]
    four = tuple(numbers)
    dotted = ".".join(str(n) for n in four)
    strings = {
        "CompanyName": "NOAA JetStream",
        "FileDescription": "JetStream server",
        "FileVersion": dotted,
        "InternalName": "jetstream-server",
        "LegalCopyright": "NOAA JetStream",
        "OriginalFilename": "jetstream-server.exe",
        "ProductName": "JetStream",
        "ProductVersion": version_text(version),
    }
    table = ", ".join(f"StringStruct({k!r}, {v!r})" for k, v in strings.items())
    text = f"""VSVersionInfo(
  ffi=FixedFileInfo(filevers={four}, prodvers={four}, mask=0x3f, flags=0x0, OS=0x40004, fileType=0x1, subtype=0x0, date=(0, 0)),
  kids=[
    StringFileInfo([StringTable('040904B0', [{table}])]),
    VarFileInfo([VarStruct('Translation', [1033, 1200])])
  ]
)
"""
    BUILD.mkdir(parents=True, exist_ok=True)
    path = BUILD / "version_info.txt"
    path.write_text(text, encoding="utf-8")
    return path


def pyinstaller(mode: str, distpath: Path) -> None:
    sep = ";" if sys.platform == "win32" else ":"
    cmd = [
        sys.executable, "-m", "PyInstaller",
        "--noconfirm",
        "--clean",
        mode,
        # UPX-packed programs are a classic false-positive for virus scanners.
        "--noupx",
        "--name", "jetstream-server",
        "--distpath", str(distpath),
        "--workpath", str(BUILD / "work"),
        "--specpath", str(BUILD),
        "--paths", str(REPO),
        "--add-data", f"{REPO / 'jetstream' / 'static'}{sep}jetstream/static",
        "--collect-submodules", "jetstream",
        "--collect-submodules", "uvicorn",
        "--hidden-import", "jetstream.main",
        "--collect-data", "googleapiclient",
        "--copy-metadata", "google-cloud-storage",
    ]
    if sys.platform == "win32":
        cmd += ["--version-file", str(windows_version_file()), "--icon", str(DESKTOP / "src-tauri" / "icons" / "icon.ico")]
    cmd.append(str(DESKTOP / "sidecar" / "jetstream_server.py"))
    # Console build on purpose: Tauri starts sidecars without a window
    # (CREATE_NO_WINDOW) and reads their output for the startup error page.
    print("Running:", " ".join(cmd))
    subprocess.run(cmd, check=True, cwd=REPO)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--portable", action="store_true", help="also build the folder version for the portable zip")
    args = parser.parse_args()
    triple = rust_target_triple()
    exe_suffix = ".exe" if sys.platform == "win32" else ""

    pyinstaller("--onefile", BUILD / "dist")
    if args.portable:
        pyinstaller("--onedir", BUILD / "portable")
        print(f"Portable server folder ready: {BUILD / 'portable' / 'jetstream-server'}")

    BINARIES.mkdir(parents=True, exist_ok=True)
    built = BUILD / "dist" / f"jetstream-server{exe_suffix}"
    target = BINARIES / f"jetstream-server-{triple}{exe_suffix}"
    shutil.copy2(built, target)
    print(f"Sidecar ready: {target}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
