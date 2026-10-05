# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "arraylake",
#   "icechunk>=2",
#   "zarr",
#   "numpy",
# ]
# ///
"""
Create a test repo whose history exercises every kind of chunk change, for
checking `core-drill object` and `core-drill chunk-changes` end to end.

Commits on `main`, oldest first:
  1. /native: write all four native chunks
  2. /native: overwrite chunk (0, 0)
  3. /native: delete chunk (1, 1)
  4. /virtual: 1200 virtual refs, half with an ETag checksum and half with a
     last-modified checksum. 1200 exceeds icechunk's default
     `min_num_chunks` (1000), so the manifest stores `compressed_location`
     with a zstd dictionary instead of plain `location` strings.
  5. /virtual: delete the virtual ref at (7,)

Icechunk repo goes to test-data/chunk-lifecycle-repo/, or to an Arraylake
repo with `--arraylake org/name` (the published copy is
iansorg/icechunk-format-examples). The virtual refs point at a placeholder
bucket, so only their metadata is meaningful.
"""

from __future__ import annotations

import argparse
import datetime
import shutil
from pathlib import Path

import icechunk
import numpy as np
import zarr

ROOT = Path(__file__).resolve().parent.parent
REPO_DIR = ROOT / "test-data" / "chunk-lifecycle-repo"
VIRTUAL_PREFIX = "s3://example-bucket/model-output/"

parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
parser.add_argument("--arraylake", metavar="ORG/NAME", help="create the repo on Arraylake instead of locally")
args = parser.parse_args()

config = icechunk.RepositoryConfig.default()
config.set_virtual_chunk_container(
    icechunk.VirtualChunkContainer(
        VIRTUAL_PREFIX,
        icechunk.s3_store(region="us-east-1", anonymous=True),
    )
)
if args.arraylake:
    import arraylake

    repo = arraylake.Client().create_repo(
        args.arraylake,
        config=config,
        description="Example history for inspecting Icechunk metadata: chunk writes, "
        "an overwrite, deletions, and virtual refs with checksums.",
    )
else:
    shutil.rmtree(REPO_DIR, ignore_errors=True)
    REPO_DIR.mkdir(parents=True)
    repo = icechunk.Repository.create(icechunk.local_filesystem_storage(str(REPO_DIR)), config=config)


def commit(message: str, edit) -> str:
    session = repo.writable_session("main")
    edit(zarr.open_group(session.store, mode="a"))
    snap = session.commit(message)
    print(f"{snap}  {message}")
    return snap


# 64x64 float64 chunks are 32 KiB, well above the inline threshold, so they
# are stored as native chunk files.
def write_native(root):
    arr = root.create_array("native", shape=(128, 128), chunks=(64, 64), dtype="f8", fill_value=0.0)
    arr[:] = np.arange(128 * 128, dtype="f8").reshape(128, 128) + 1


def overwrite_chunk(root):
    root["native"][0:64, 0:64] = -1.0


# zarr's default write_empty_chunks=False removes a chunk that is entirely
# fill value, which icechunk records as a deleted chunk ref.
def delete_chunk(root):
    root["native"][64:128, 64:128] = 0.0


N_VIRTUAL = 1200


def write_virtual(root):
    root.create_array("virtual", shape=(N_VIRTUAL,), chunks=(1,), dtype="f4", fill_value=0.0)


def set_virtual_refs(store):
    modified = datetime.datetime(2024, 6, 1, 12, 0, tzinfo=datetime.UTC)
    for i in range(N_VIRTUAL):
        checksum = f'"etag-{i:05d}"' if i % 2 == 0 else modified
        store.set_virtual_ref(
            f"virtual/c/{i}",
            f"{VIRTUAL_PREFIX}run-{i // 100:02d}/file-{i:05d}.nc",
            offset=i * 4,
            length=4,
            checksum=checksum,
        )


def delete_virtual_ref(root):
    root["virtual"][7] = 0.0


commit("Write all native chunks", write_native)
commit("Overwrite native chunk (0, 0)", overwrite_chunk)
commit("Delete native chunk (1, 1)", delete_chunk)

session = repo.writable_session("main")
write_virtual(zarr.open_group(session.store, mode="a"))
set_virtual_refs(session.store)
print(f"{session.commit('Add 1200 virtual refs with checksums')}  Add virtual refs")

commit("Delete virtual ref (7,)", delete_virtual_ref)

print(f"\nRepo written to {args.arraylake or REPO_DIR}")
