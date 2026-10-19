"""Build the JetStream upload engine (Rust) and place it where Tauri bundles sidecars.

    python desktop/scripts/build_engine.py

Produces src-tauri/binaries/jetstream-transfer-<rust target triple>[.exe]. In the installed
app it sits next to the bundled server, which finds it automatically.
"""

import shutil
import subprocess
import sys
from pathlib import Path

from build_sidecar import BINARIES, rust_target_triple

DESKTOP = Path(__file__).resolve().parent.parent
ENGINE = DESKTOP / "transfer"


def main() -> int:
    subprocess.run(["cargo", "build", "--release", "--manifest-path", str(ENGINE / "Cargo.toml")], check=True)
    exe = ".exe" if sys.platform == "win32" else ""
    BINARIES.mkdir(parents=True, exist_ok=True)
    target = BINARIES / f"jetstream-transfer-{rust_target_triple()}{exe}"
    shutil.copy2(ENGINE / "target" / "release" / f"jetstream-transfer{exe}", target)
    print(f"Engine ready: {target}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
