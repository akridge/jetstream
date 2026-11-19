"""Desktop release tooling: release notes come from desktop/CHANGELOG.md."""

import importlib.util
import json
from pathlib import Path

import pytest

DESKTOP = Path(__file__).resolve().parent.parent / "desktop"


def load(name):
    spec = importlib.util.spec_from_file_location(name, DESKTOP / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


release_notes = load("release_notes")

SAMPLE = """# Changes

## 0.10.0
- Ten

## 0.2.1

### Uploads
- Faster

## 0.2.0
- First
"""


def test_section_for_version():
    assert release_notes.notes_for(SAMPLE, "0.2.1") == "### Uploads\n- Faster\n"
    assert release_notes.notes_for(SAMPLE, "desktop-v0.2.0") == "- First\n"
    assert release_notes.notes_for(SAMPLE, "v0.10.0") == "- Ten\n"


def test_missing_or_empty_section_fails():
    with pytest.raises(ValueError):
        release_notes.notes_for(SAMPLE, "0.3.0")
    with pytest.raises(ValueError):
        release_notes.notes_for("## 0.4.0\n\n## 0.3.0\n- x\n", "0.4.0")
    # 0.2 is not 0.2.0 (no prefix matches).
    with pytest.raises(ValueError):
        release_notes.notes_for(SAMPLE, "0.2")


def test_current_version_has_notes():
    version = json.loads((DESKTOP / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))["version"]
    notes = release_notes.notes_for((DESKTOP / "CHANGELOG.md").read_text(encoding="utf-8"), version)
    assert notes.strip()
