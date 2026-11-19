"""The desktop catalog tool (Rust engine) must give the same catalogs as the Python one.

Runs the Python Dataset Creator code and `jetstream-transfer catalog --names ...` over the same
generated object names with many option combinations and compares:
- the OASIS export, byte for byte
- flat and grouped lists, site instances, group summaries and counts

Skipped when the engine hasn't been built (cargo build in desktop/transfer).
"""

import json
import random
import subprocess
from pathlib import Path

import pytest

from jetstream.services_dataset import (
    DatasetCatalogRequest,
    build_catalog,
    build_gcs_uris,
    list_media_blobs,
    to_export_payload,
)

ROOT = Path(__file__).resolve().parent.parent
ENGINES = [ROOT / "desktop" / "transfer" / "target" / kind / name for kind in ("release", "debug") for name in ("jetstream-transfer", "jetstream-transfer.exe")]
ENGINE = next((p for p in ENGINES if p.exists()), None)
pytestmark = pytest.mark.skipif(ENGINE is None, reason="jetstream-transfer not built")


def make_names(n=2000, seed=7):
    rng = random.Random(seed)
    sites = ["site_A", "site_B", "Kona", "HA2401", "reef-03", "Montérey", "x y"]
    folders = ["", "raw/", "thumbs/", "Thumbs/", "deep/er/", "2024/03/", "Ünïcode/", "a//b/"]
    exts = [".jpg", ".JPG", ".jpeg", ".png", ".tif", ".TIFF", ".bmp", ".webp", ".gif", ".mp4", ".MOV", ".nc", ".csv", "", ".tar.gz"]
    names = set()
    while len(names) < n:
        stem = "_".join(
            [rng.choice(sites), f"{rng.randint(0, 40):03d}", rng.choice(["2023", "2024", "", "cam1"])]
        ).strip("_")
        if rng.random() < 0.05:
            stem = "." + stem
        if rng.random() < 0.05:
            stem = stem.replace("_", "__")
        names.add(f"survey/{rng.choice(folders)}{stem}{rng.choice(exts)}")
    names.update(["survey/", "survey/folder/", "survey/.bashrc", "other/site_A_001.jpg", "survey/site_Z_1.jpg"])
    return sorted(names)


class _Blob:
    def __init__(self, name):
        self.name = name


class _Bucket:
    def __init__(self, names):
        self.names = names

    def list_blobs(self, prefix="", max_results=None):
        return [_Blob(n) for n in self.names if n.startswith(prefix)]


class _Client:
    def __init__(self, names):
        self.names = names

    def bucket(self, _name):
        return _Bucket(self.names)


def python_catalog(names, request):
    req = DatasetCatalogRequest(**request)
    from jetstream.services_dataset import resolve_extensions

    exts = resolve_extensions(req.include_images, req.include_videos, req.extensions)
    matches, _ = list_media_blobs(
        _Client(names),
        req.gcs_prefix,
        include_images=req.include_images,
        include_videos=req.include_videos,
        extensions=exts,
        filename_filter=req.filename_filter,
        folder_filter=req.folder_filter,
        ignore_folder_names=req.ignore_folder_names,
        max_path_depth=req.max_path_depth,
        scan_ceiling=req.max_results or 50000,
    )
    catalog = build_catalog(req, build_gcs_uris(req.gcs_prefix, matches)).response
    return catalog, json.dumps(to_export_payload(catalog), indent=2)


def rust_catalog(tmp_path, names, request, fmt):
    names_file = tmp_path / "names.txt"
    names_file.write_text("\n".join(names), encoding="utf-8")
    out = tmp_path / f"out.{fmt}"
    subprocess.run(
        [str(ENGINE), "catalog", request["gcs_prefix"], "--request", json.dumps(request), "--names", str(names_file), "--format", fmt, "--out", str(out)],
        check=True,
        capture_output=True,
    )
    return out.read_text(encoding="utf-8")


REQUESTS = [
    {},
    {"include_videos": False},
    {"include_images": False},
    {"extensions": ["JPG", ".png", "gif", ".nc"]},
    {"filename_token_count": 1},
    {"filename_token_count": 2, "filename_delimiter": "-"},
    {"filename_delimiter": "__", "filename_token_count": 1},
    {"group_by_mode": "none"},
    {"group_by_mode": "folder_segment", "folder_segment_index": 0},
    {"group_by_mode": "folder_segment", "folder_segment_index": 1},
    {"group_by_mode": "folder_segment", "folder_segment_index": 9},
    {"group_by_mode": "regex", "regex_pattern": r"site_([A-Z])_(\d+)", "regex_group_index": 2},
    {"group_by_mode": "regex", "regex_pattern": r"(cam1)?(\d{3})", "regex_group_index": 1},
    {"group_by_mode": "regex", "regex_pattern": r"[A-Z]+", "regex_group_index": 5},
    {"group_by_mode": "regex"},
    {"group_by_mode": "manual", "manual_overrides": {"kona": ["kona"], "A": ["*site_a_0[0-1]*"], "tif": ["*.tif?"]}},
    {"group_by_mode": "manual", "manual_overrides": {}},
    {"filename_filter": "site_A"},
    {"filename_filter": "*_00?_*.jpg"},
    {"filename_filter": "[!s]*"},
    {"folder_filter": "DEEP"},
    {"ignore_folder_names": ["thumbs", "RAW"]},
    {"ignore_folder_names": "thumbs, b"},
    {"max_path_depth": 0},
    {"max_path_depth": 1},
    {"max_results": 37},
    {"selected_groups": ["Kona_001", "site_A_002_2024", "nope"]},
    {"site_instance_mode": "threshold", "site_instance_threshold": 3},
    {"site_instance_mode": "manual", "manual_site_groups": ["Kona_010_2024", "x"]},
    {"group_by_mode": "folder_segment", "folder_segment_index": 1, "site_instance_mode": "folder_boundary", "site_instance_folder_segment_index": 1},
    {"site_instance_mode": "filename_pattern", "site_instance_pattern": r"_00[0-3]_"},
    {"site_instance_mode": "filename_pattern", "site_instance_pattern": "(unclosed"},
    {"catalog_format": "flat"},
    {"catalog_format": "grouped", "group_by_mode": "folder_segment"},
    {"gcs_prefix": "gs://bucket/"},
    {"gcs_prefix": "gs://bucket/survey/deep"},
]


@pytest.mark.parametrize("extra", REQUESTS, ids=[json.dumps(r)[:60] for r in REQUESTS])
def test_rust_catalog_matches_python(tmp_path, extra):
    names = make_names()
    request = {"gcs_prefix": "gs://bucket/survey", **extra}
    py_catalog, py_oasis = python_catalog(names, request)

    assert rust_catalog(tmp_path, names, request, "oasis") == py_oasis

    rust = json.loads(rust_catalog(tmp_path, names, request, "catalog"))
    py = py_catalog.model_dump(mode="json")
    for key in ("gcs_prefix", "flat_instances", "grouped_instances", "site_instances", "groups", "catalog_format"):
        assert rust[key] == py[key], key
    for key in ("group_by_mode", "site_instance_mode", "total_items", "total_groups"):
        assert rust["metadata"][key] == py["metadata"][key], key


def test_desktop_kinds_extend_the_python_defaults(tmp_path):
    names = make_names()
    base = {"gcs_prefix": "gs://bucket/survey"}
    images_only = json.loads(rust_catalog(tmp_path, names, {**base, "kinds": ["images"]}, "catalog"))
    _, py_oasis = python_catalog(names, {**base, "include_videos": False})
    assert rust_catalog(tmp_path, names, {**base, "kinds": ["images"]}, "oasis") == py_oasis
    everything = json.loads(rust_catalog(tmp_path, names, {**base, "kinds": ["all"]}, "catalog"))
    assert everything["metadata"]["total_items"] > images_only["metadata"]["total_items"]
    assert everything["metadata"]["extensions_used"] == []
