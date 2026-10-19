"""Transfer preferences shared by the web and desktop apps (stored in the data folder)."""

import json
import threading
from typing import Dict

from .config import user_data_dir

DEFAULTS: Dict = {"max_mbps": 0.0}
_lock = threading.Lock()


def _path():
    return user_data_dir() / "transfer-settings.json"


def load() -> Dict:
    try:
        with open(_path(), "r", encoding="utf-8") as fh:
            stored = json.load(fh)
    except (OSError, ValueError):
        stored = {}
    merged = dict(DEFAULTS)
    if isinstance(stored, dict):
        merged.update({k: stored[k] for k in DEFAULTS if k in stored})
    try:
        merged["max_mbps"] = max(0.0, float(merged["max_mbps"]))
    except (TypeError, ValueError):
        merged["max_mbps"] = 0.0
    return merged


def save(changes: Dict) -> Dict:
    with _lock:
        current = load()
        current.update({k: v for k, v in changes.items() if k in DEFAULTS})
        current["max_mbps"] = max(0.0, float(current["max_mbps"] or 0))
        path = _path()
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".tmp")
        tmp.write_text(json.dumps(current, indent=2), encoding="utf-8")
        tmp.replace(path)
        return current
