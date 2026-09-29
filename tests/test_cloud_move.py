import asyncio

import pytest
from fastapi import BackgroundTasks, HTTPException
from sqlalchemy import create_engine
from sqlalchemy.orm import sessionmaker

from jetstream.database import Base, UploadJob
from jetstream.routers import cloud_analyzer


class FakeBlob:
    def __init__(self, bucket, name, size=1, crc32c="crc", generation=1, present=True, reload_error=False):
        self.bucket = bucket
        self.name = name
        self.size = size
        self.crc32c = crc32c
        self.generation = generation
        self.present = present
        self.reload_error = reload_error

    def reload(self):
        if self.reload_error:
            raise RuntimeError(f"Reload failed: {self.name}")
        if not self.present:
            raise RuntimeError(f"Object not found: {self.name}")

    def exists(self):
        return self.present

    def delete(self, if_generation_match=None):
        assert if_generation_match == self.generation
        self.bucket.events.append(("delete", self.bucket.name, self.name))
        self.present = False


class FakeBucket:
    def __init__(self, name, events, bad_copy=False, bad_reload=False):
        self.name = name
        self.events = events
        self.bad_copy = bad_copy
        self.bad_reload = bad_reload
        self.objects = {}

    def add(self, name, size=1, crc32c="crc", generation=1):
        blob = FakeBlob(self, name, size, crc32c, generation)
        self.objects[name] = blob
        return blob

    def blob(self, name):
        return self.objects.get(name) or FakeBlob(self, name, present=False)

    def list_blobs(self, prefix):
        return [blob for name, blob in self.objects.items() if blob.present and name.startswith(prefix)]

    def copy_blob(
        self,
        source_blob,
        destination_bucket,
        new_name,
        if_generation_match=None,
        if_source_generation_match=None,
    ):
        assert if_generation_match == 0
        assert if_source_generation_match == source_blob.generation
        assert not destination_bucket.blob(new_name).exists()
        self.events.append(("copy", source_blob.name, new_name))
        crc32c = "bad-crc" if destination_bucket.bad_copy else source_blob.crc32c
        copied = destination_bucket.add(new_name, source_blob.size, crc32c, generation=99)
        copied.reload_error = destination_bucket.bad_reload
        return copied


class FakeClient:
    def __init__(self, *buckets):
        self.buckets = {bucket.name: bucket for bucket in buckets}

    def bucket(self, name):
        return self.buckets[name]


@pytest.fixture
def move_environment(monkeypatch):
    events = []
    source = FakeBucket("source", events)
    destination = FakeBucket("destination", events)
    client = FakeClient(source, destination)
    monkeypatch.setattr(cloud_analyzer, "get_storage_client", lambda: client)
    monkeypatch.setattr(cloud_analyzer, "update_cloud_operation_job", lambda *args, **kwargs: None)
    cloud_analyzer.cloud_transfer_jobs["test-job"] = {}
    yield events, source, destination
    cloud_analyzer.cloud_transfer_jobs.pop("test-job", None)


def test_move_path_and_prefix_mapping_are_explicit():
    assert cloud_analyzer.parse_move_path("gs://bucket/file.csv", "object") == (
        "bucket",
        "file.csv",
        "gs://bucket/file.csv",
    )
    assert cloud_analyzer.parse_move_path("gs://bucket/incoming/", "prefix")[1] == "incoming/"
    assert (
        cloud_analyzer.destination_name_for_move("incoming/sub/file.csv", "incoming/", "archive/")
        == "archive/sub/file.csv"
    )

    with pytest.raises(ValueError, match="begin with gs://"):
        cloud_analyzer.parse_move_path("bucket/file.csv", "object")
    with pytest.raises(ValueError, match="must end with"):
        cloud_analyzer.parse_move_path("gs://bucket/incoming", "prefix")
    with pytest.raises(ValueError, match="Choose a folder option"):
        cloud_analyzer.parse_move_path("gs://bucket/incoming/", "object")


def test_legacy_and_move_jobs_serialize_with_correct_operation():
    sync = UploadJob(
        job_id="sync",
        source_path="gs://a/source",
        destination_bucket="b",
        filters={"job_type": "cloud_sync", "transfer_direction": "cloud_to_cloud"},
    )
    move = UploadJob(
        job_id="move",
        source_path="gs://a/source",
        destination_bucket="b",
        filters={"job_type": "cloud_move", "operation": "move", "source_kind": "object"},
    )

    assert cloud_analyzer.cloud_operation_job_to_transfer(sync)["operation"] == "sync"
    assert cloud_analyzer.cloud_operation_job_to_transfer(move)["operation"] == "move"


def test_collision_preflight_makes_no_changes(move_environment):
    events, source, destination = move_environment
    source.add("incoming/a.txt")
    destination.add("archive/a.txt")

    with pytest.raises(RuntimeError, match="Destination already contains"):
        cloud_analyzer._run_cloud_move_sync(
            "test-job",
            "gs://source/incoming/",
            "gs://destination/archive/",
            "prefix",
        )

    assert events == []
    assert source.blob("incoming/a.txt").present


def test_verification_failure_rolls_back_and_preserves_source(move_environment):
    events, source, destination = move_environment
    source.add("incoming/a.txt", size=5)
    destination.bad_copy = True

    with pytest.raises(RuntimeError, match="Copy verification failed"):
        cloud_analyzer._run_cloud_move_sync(
            "test-job",
            "gs://source/incoming/",
            "gs://destination/archive/",
            "prefix",
        )

    assert events == [
        ("copy", "incoming/a.txt", "archive/a.txt"),
        ("delete", "destination", "archive/a.txt"),
    ]
    assert source.blob("incoming/a.txt").present
    assert not destination.blob("archive/a.txt").present


def test_reload_failure_after_copy_rolls_back_destination(move_environment):
    events, source, destination = move_environment
    source.add("incoming/a.txt", size=5)
    destination.bad_reload = True

    with pytest.raises(RuntimeError, match="Reload failed"):
        cloud_analyzer._run_cloud_move_sync(
            "test-job",
            "gs://source/incoming/",
            "gs://destination/archive/",
            "prefix",
        )

    assert events == [
        ("copy", "incoming/a.txt", "archive/a.txt"),
        ("delete", "destination", "archive/a.txt"),
    ]
    assert source.blob("incoming/a.txt").present


def test_success_copies_all_objects_before_source_deletion(move_environment):
    events, source, destination = move_environment
    source.add("incoming/a.txt", size=5, generation=10)
    source.add("incoming/sub/b.txt", size=7, generation=11)

    cloud_analyzer._run_cloud_move_sync(
        "test-job",
        "gs://source/incoming/",
        "gs://destination/archive/",
        "prefix",
    )

    assert events[:2] == [
        ("copy", "incoming/a.txt", "archive/a.txt"),
        ("copy", "incoming/sub/b.txt", "archive/sub/b.txt"),
    ]
    assert events[2:] == [
        ("delete", "source", "incoming/a.txt"),
        ("delete", "source", "incoming/sub/b.txt"),
    ]
    assert cloud_analyzer.cloud_transfer_jobs["test-job"]["status"] == "completed"


def test_dry_run_reports_plan_without_copying_or_deleting(move_environment):
    events, source, destination = move_environment
    source.add("incoming/a.txt", size=5)
    source.add("incoming/sub/b.txt", size=7)

    cloud_analyzer._run_cloud_move_sync(
        "test-job",
        "gs://source/incoming/",
        "gs://destination/archive/",
        "prefix",
        dry_run=True,
    )

    assert events == []
    assert source.blob("incoming/a.txt").present
    assert not destination.blob("archive/a.txt").present

    job = cloud_analyzer.cloud_transfer_jobs["test-job"]
    assert job["status"] == "completed"
    assert "DRY RUN: 2 object(s), 12 bytes would move." in job["output"]
    assert "incoming/sub/b.txt -> archive/sub/b.txt" in job["output"]


def test_dry_run_still_fails_on_destination_conflicts(move_environment):
    events, source, destination = move_environment
    source.add("incoming/a.txt")
    destination.add("archive/a.txt")

    with pytest.raises(RuntimeError, match="Destination already contains"):
        cloud_analyzer._run_cloud_move_sync(
            "test-job",
            "gs://source/incoming/",
            "gs://destination/archive/",
            "prefix",
            dry_run=True,
        )

    assert events == []


def test_move_endpoint_persists_job_and_registers_background_task():
    engine = create_engine("sqlite:///:memory:")
    Base.metadata.create_all(engine)
    db = sessionmaker(bind=engine)()
    background_tasks = BackgroundTasks()

    response = asyncio.run(
        cloud_analyzer.start_cloud_move(
            cloud_analyzer.CloudMoveRequest(
                source_path="gs://source/data/file.csv",
                dest_path="gs://destination/archive/file.csv",
                source_kind="object",
            ),
            background_tasks,
            db,
        )
    )

    job = db.query(UploadJob).filter(UploadJob.job_id == response.job_id).one()
    assert job.filters["job_type"] == "cloud_move"
    assert job.filters["phase"] == "preflight"
    assert job.no_clobber is True
    assert len(background_tasks.tasks) == 1
    cloud_analyzer.cloud_transfer_jobs.pop(response.job_id, None)
    db.close()


def test_move_endpoint_rejects_destination_inside_source_prefix():
    engine = create_engine("sqlite:///:memory:")
    Base.metadata.create_all(engine)
    db = sessionmaker(bind=engine)()

    with pytest.raises(HTTPException, match="Destination folder cannot be inside"):
        asyncio.run(
            cloud_analyzer.start_cloud_move(
                cloud_analyzer.CloudMoveRequest(
                    source_path="gs://bucket/data/",
                    dest_path="gs://bucket/data/archive/",
                    source_kind="folder_contents",
                ),
                BackgroundTasks(),
                db,
            )
        )
    db.close()


def test_whole_folder_move_nests_source_folder_in_destination():
    engine = create_engine("sqlite:///:memory:")
    Base.metadata.create_all(engine)
    db = sessionmaker(bind=engine)()

    response = asyncio.run(
        cloud_analyzer.start_cloud_move(
            cloud_analyzer.CloudMoveRequest(
                source_path="gs://source/incoming/",
                dest_path="gs://destination/archive/",
                source_kind="folder",
            ),
            BackgroundTasks(),
            db,
        )
    )

    job = db.query(UploadJob).filter(UploadJob.job_id == response.job_id).one()
    assert job.destination_path == "archive/incoming/"
    assert job.filters["source_kind"] == "folder"
    cloud_analyzer.cloud_transfer_jobs.pop(response.job_id, None)
    db.close()


def test_folder_contents_move_keeps_destination_flat():
    engine = create_engine("sqlite:///:memory:")
    Base.metadata.create_all(engine)
    db = sessionmaker(bind=engine)()

    response = asyncio.run(
        cloud_analyzer.start_cloud_move(
            cloud_analyzer.CloudMoveRequest(
                source_path="gs://source/incoming/",
                dest_path="gs://destination/archive/",
                source_kind="folder_contents",
            ),
            BackgroundTasks(),
            db,
        )
    )

    job = db.query(UploadJob).filter(UploadJob.job_id == response.job_id).one()
    assert job.destination_path == "archive/"
    cloud_analyzer.cloud_transfer_jobs.pop(response.job_id, None)
    db.close()