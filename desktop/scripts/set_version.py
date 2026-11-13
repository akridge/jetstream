"""Set or check the JetStream Desktop version.

The desktop app (with its bundled upload engine) has its own version, separate from the
`jetstream` Python package. src-tauri/tauri.conf.json is where it lives; the other files follow:

    desktop/package.json (and package-lock.json), desktop/src-tauri/Cargo.toml,
    desktop/transfer/Cargo.toml

    python desktop/scripts/set_version.py 0.2.1          # set it everywhere
    python desktop/scripts/set_version.py --check        # fail if the files disagree
    python desktop/scripts/set_version.py --check --tag desktop-v0.2.1   # ...or don't match a tag
    python desktop/scripts/set_version.py --show         # print the version

Builds also carry a build number and commit (JETSTREAM_BUILD / JETSTREAM_COMMIT, set by CI).
"""

import argparse
import json
import re
import sys
from pathlib import Path

DESKTOP = Path(__file__).resolve().parent.parent
TAURI_CONF = DESKTOP / "src-tauri" / "tauri.conf.json"
PACKAGE_JSON = DESKTOP / "package.json"
PACKAGE_LOCK = DESKTOP / "package-lock.json"
CARGO_FILES = [DESKTOP / "src-tauri" / "Cargo.toml", DESKTOP / "transfer" / "Cargo.toml"]
VERSION_RE = re.compile(r"^\d+\.\d+\.\d+$")
# The [package] version line (the first `version = "..."` in the file).
CARGO_VERSION_RE = re.compile(r'^(version\s*=\s*")([^"]+)(")', re.MULTILINE)


def current() -> dict:
    found = {
        str(TAURI_CONF.relative_to(DESKTOP)): json.loads(TAURI_CONF.read_text(encoding="utf-8"))["version"],
        str(PACKAGE_JSON.relative_to(DESKTOP)): json.loads(PACKAGE_JSON.read_text(encoding="utf-8"))["version"],
    }
    for path in CARGO_FILES:
        match = CARGO_VERSION_RE.search(path.read_text(encoding="utf-8"))
        found[str(path.relative_to(DESKTOP))] = match.group(2) if match else "?"
    return found


def set_json_version(path: Path, version: str) -> None:
    text = path.read_text(encoding="utf-8")
    # Replace only the top-level "version" value, keeping the file's formatting.
    new, count = re.subn(r'^(\s{2}"version":\s*")[^"]*(")', rf"\g<1>{version}\g<2>", text, count=1, flags=re.MULTILINE)
    if not count:
        raise SystemExit(f"No version in {path}")
    path.write_text(new, encoding="utf-8")


def set_version(version: str) -> None:
    if not VERSION_RE.match(version):
        raise SystemExit(f"Use MAJOR.MINOR.PATCH (e.g. 0.2.1), not {version!r}: Windows installers need plain numbers.")
    set_json_version(TAURI_CONF, version)
    set_json_version(PACKAGE_JSON, version)
    if PACKAGE_LOCK.exists():
        lock = json.loads(PACKAGE_LOCK.read_text(encoding="utf-8"))
        lock["version"] = version
        lock.get("packages", {}).get("", {})["version"] = version
        PACKAGE_LOCK.write_text(json.dumps(lock, indent=2) + "\n", encoding="utf-8")
    for path in CARGO_FILES:
        text = path.read_text(encoding="utf-8")
        path.write_text(CARGO_VERSION_RE.sub(rf"\g<1>{version}\g<3>", text, count=1), encoding="utf-8")
    print(f"JetStream Desktop version set to {version} (Cargo.lock files update on the next build).")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("version", nargs="?", help="new version, e.g. 0.2.1")
    parser.add_argument("--check", action="store_true", help="fail if the files disagree")
    parser.add_argument("--tag", help="with --check: a release tag (desktop-vX.Y.Z) the version must match")
    parser.add_argument("--show", action="store_true", help="print the version")
    args = parser.parse_args()

    if args.version:
        set_version(args.version)
    found = current()
    versions = set(found.values())
    if args.show:
        print(found[str(TAURI_CONF.relative_to(DESKTOP))])
    if args.check:
        if len(versions) != 1:
            print("Desktop version files disagree:", file=sys.stderr)
            for name, value in found.items():
                print(f"  {name}: {value}", file=sys.stderr)
            print("Fix with: python desktop/scripts/set_version.py X.Y.Z", file=sys.stderr)
            return 1
        version = versions.pop()
        if args.tag:
            tagged = args.tag.removeprefix("refs/tags/").removeprefix("desktop-v")
            if tagged != version:
                print(f"Tag {args.tag} doesn't match the desktop version {version}. "
                      f"Run: python desktop/scripts/set_version.py {tagged}, commit, and tag again.", file=sys.stderr)
                return 1
        print(f"Desktop version {version}: all files agree" + (f" with {args.tag}" if args.tag else ""))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
