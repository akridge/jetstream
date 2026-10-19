#!/usr/bin/env bash
# Compare jetstream-transfer with `gcloud storage rsync` on a real bucket.
#
#   desktop/transfer/bench/bench.sh gs://YOUR-TEST-BUCKET/jetstream-bench
#
# Needs: gcloud signed in (`gcloud auth login` and `gcloud auth application-default login`),
# write access to the bucket, ~6 GB free disk. Objects are written under the given prefix;
# delete them afterwards with: gcloud storage rm -r gs://YOUR-TEST-BUCKET/jetstream-bench
set -euo pipefail

DEST=${1:?usage: bench.sh gs://bucket/prefix}
HERE=$(cd "$(dirname "$0")/.." && pwd)
WORK=${BENCH_DIR:-$(mktemp -d)}
SMALL=${SMALL_FILES:-20000}
LARGE=${LARGE_FILES:-5}
LARGE_MB=${LARGE_MB:-1024}

cargo build --release --manifest-path "$HERE/Cargo.toml" >/dev/null
ENGINE="$HERE/target/release/jetstream-transfer"

echo "Creating test data in $WORK ($SMALL small files, $LARGE x ${LARGE_MB} MB)"
mkdir -p "$WORK/small" "$WORK/large"
python3 - "$WORK" "$SMALL" "$LARGE" "$LARGE_MB" <<'PY'
import os, sys
work, small, large, large_mb = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
for i in range(small):
    d = os.path.join(work, "small", f"d{i // 1000:03d}")
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, f"f{i:06d}.jpg"), "wb") as f:
        f.write(os.urandom(4096 + (i % 64) * 1024))
for i in range(large):
    p = os.path.join(work, "large", f"big{i}.bin")
    if not os.path.exists(p):
        with open(p, "wb") as f:
            for _ in range(large_mb):
                f.write(os.urandom(1 << 20))
PY

timed() {
  local label=$1; shift
  local start end
  start=$(date +%s.%N)
  "$@" >/dev/null 2>&1 || echo "  ($label exited with $?)"
  end=$(date +%s.%N)
  printf "%-45s %8.1f s\n" "$label" "$(echo "$end - $start" | bc)"
}

for set in small large; do
  echo
  echo "== $set files"
  timed "gcloud rsync (first upload)"   gcloud storage rsync --recursive --checksums-only "$WORK/$set" "$DEST/gcloud/$set"
  timed "gcloud rsync (nothing changed)" gcloud storage rsync --recursive --checksums-only "$WORK/$set" "$DEST/gcloud/$set"
  rm -f "$WORK/manifest.db"*
  timed "jetstream-transfer (first upload)"   "$ENGINE" sync "$WORK/$set" "$DEST/engine/$set" --manifest "$WORK/manifest.db"
  timed "jetstream-transfer (nothing changed)" "$ENGINE" sync "$WORK/$set" "$DEST/engine/$set" --manifest "$WORK/manifest.db"
  timed "jetstream-transfer (--verify)"        "$ENGINE" sync "$WORK/$set" "$DEST/engine/$set" --manifest "$WORK/manifest.db" --verify
done
echo
echo "Test data left in $WORK; remove the uploaded objects with: gcloud storage rm -r $DEST"
