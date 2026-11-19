"""Print one version's release notes from desktop/CHANGELOG.md.

    python desktop/scripts/release_notes.py 0.2.0            # print them
    python desktop/scripts/release_notes.py desktop-v0.2.0 -o notes.md

Fails if the changelog has no section for that version, so a release can't go out without notes.
"""

import argparse
import re
import sys
from pathlib import Path

CHANGELOG = Path(__file__).resolve().parent.parent / "CHANGELOG.md"


def notes_for(text: str, version: str) -> str:
    """The body of the `## <version>` section (up to the next `## ` heading)."""
    version = version.removeprefix("desktop-v").removeprefix("v")
    lines = text.splitlines()
    heading = re.compile(rf"^##\s+v?{re.escape(version)}(\s|$)")
    for start, line in enumerate(lines):
        if heading.match(line):
            body = []
            for line in lines[start + 1 :]:
                if line.startswith("## "):
                    break
                body.append(line)
            notes = "\n".join(body).strip()
            if notes:
                return notes + "\n"
            break
    raise ValueError(f"No notes for {version} in CHANGELOG.md")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("version")
    parser.add_argument("-o", "--output", type=Path)
    args = parser.parse_args()
    try:
        notes = notes_for(CHANGELOG.read_text(encoding="utf-8"), args.version)
    except ValueError as err:
        print(err, file=sys.stderr)
        return 1
    if args.output:
        args.output.write_text(notes, encoding="utf-8")
    else:
        sys.stdout.write(notes)
    return 0


if __name__ == "__main__":
    sys.exit(main())
