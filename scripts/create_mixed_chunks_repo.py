# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "icechunk",
#   "zarr",
#   "numpy",
# ]
# ///
"""
Create a test Icechunk repository demonstrating ALL THREE chunk types
(inline, native/stored, and virtual) in the same array across multiple commits.

Source binary files go to:
  test-data/mixed-chunks-source-a/  (obs station A, months 0-1)
  test-data/mixed-chunks-source-b/  (obs station B, months 2-3)
Icechunk repo goes to: test-data/mixed-chunks-repo/

Array layout:  /climate_data  shape=(12, 4)  chunks=(1, 4)
  Months 0-1  → virtual chunks  (file:// source-a)
  Months 2-3  → virtual chunks  (file:// source-b)
  Months 4-7  → native/stored chunks  (written via zarr, stored in icechunk)
  Months 8-11 → inline chunks  (stored inline because bytes <= threshold)

Commit history:
  Commit 1: Create array skeleton + add virtual chunks for months 0–3
  Commit 2: Add native/stored chunks for months 4–7
  Commit 3: Add inline chunks for months 8–11
"""

from __future__ import annotations

import struct
from pathlib import Path

import numpy as np
import zarr
import icechunk

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
ROOT = Path("/Users/ian/Documents/dev/core-drill")
SOURCE_DIR_A = ROOT / "test-data" / "mixed-chunks-source-a"
SOURCE_DIR_B = ROOT / "test-data" / "mixed-chunks-source-b"
REPO_DIR = ROOT / "test-data" / "mixed-chunks-repo"

SOURCE_DIR_A.mkdir(parents=True, exist_ok=True)
SOURCE_DIR_B.mkdir(parents=True, exist_ok=True)
REPO_DIR.mkdir(parents=True, exist_ok=True)

FLOAT32_BYTES = 4
VALUES_PER_CHUNK = 4  # 4 stations per month row
CHUNK_BYTES = VALUES_PER_CHUNK * FLOAT32_BYTES  # 16 bytes

# ---------------------------------------------------------------------------
# Step 1 – Write raw binary source files for the virtual chunks (months 0-3)
# ---------------------------------------------------------------------------
# Each file contains exactly 4 float32 values (16 bytes, no header).
# Months 0-1 go to source-a (obs station A).
# Months 2-3 go to source-b (obs station B).

virtual_source_data_a = {
    0: np.array([270.1, 271.2, 272.3, 273.4], dtype="<f4"),  # January
    1: np.array([274.5, 275.6, 276.7, 277.8], dtype="<f4"),  # February
}

virtual_source_data_b = {
    2: np.array([278.9, 279.0, 280.1, 281.2], dtype="<f4"),  # March
    3: np.array([282.3, 283.4, 284.5, 285.6], dtype="<f4"),  # April
}

virtual_source_data = {**virtual_source_data_a, **virtual_source_data_b}

virtual_files_a: dict[int, Path] = {}
virtual_files_b: dict[int, Path] = {}

print("Writing virtual chunk source files (obs station A):")
for month, data in virtual_source_data_a.items():
    fname = SOURCE_DIR_A / f"month_{month:02d}.bin"
    fname.write_bytes(data.tobytes())
    virtual_files_a[month] = fname
    print(f"  {fname}  ({fname.stat().st_size} bytes, {data.tolist()})")

print("Writing virtual chunk source files (obs station B):")
for month, data in virtual_source_data_b.items():
    fname = SOURCE_DIR_B / f"month_{month:02d}.bin"
    fname.write_bytes(data.tobytes())
    virtual_files_b[month] = fname
    print(f"  {fname}  ({fname.stat().st_size} bytes, {data.tolist()})")

print()

# ---------------------------------------------------------------------------
# Step 2 – Create the Icechunk repository
# ---------------------------------------------------------------------------
# inline_chunk_threshold_bytes = 20: chunks <= 20 bytes go inline.
# Our chunks are 16 bytes (4 × float32), so months 8-11 will be inlined
# provided we write them via a session with the same config.
# Months 4-7 are also 16 bytes but the threshold only matters at write time;
# since we configure the same threshold for all sessions, we need to be
# deliberate. To make months 4-7 stored (not inline) we set the threshold
# BELOW 16 bytes (i.e., 0) for their commit, and raise it for months 8-11.
#
# Strategy:
#   - Repo created with threshold=0 (nothing inline by default).
#   - Commit 2 (native): opens with threshold=0  → stored.
#   - Commit 3 (inline): opens with threshold=20 → 16-byte chunks go inline.

source_prefix_a = f"file://{SOURCE_DIR_A}"
source_prefix_b = f"file://{SOURCE_DIR_B}"


def make_config(inline_threshold: int = 0) -> icechunk.RepositoryConfig:
    cfg = icechunk.RepositoryConfig.default()
    cfg.inline_chunk_threshold_bytes = inline_threshold
    cfg.set_virtual_chunk_container(
        icechunk.VirtualChunkContainer(
            source_prefix_a + "/",
            icechunk.local_filesystem_store(str(SOURCE_DIR_A)),
        )
    )
    cfg.set_virtual_chunk_container(
        icechunk.VirtualChunkContainer(
            source_prefix_b + "/",
            icechunk.local_filesystem_store(str(SOURCE_DIR_B)),
        )
    )
    return cfg


credentials = icechunk.containers_credentials({
    source_prefix_a + "/": None,
    source_prefix_b + "/": None,
})

storage = icechunk.local_filesystem_storage(str(REPO_DIR))

repo = icechunk.Repository.create(
    storage,
    config=make_config(inline_threshold=0),
    authorize_virtual_chunk_access=credentials,
)
print(f"Created repo at {REPO_DIR}")
print(f"  inline_chunk_threshold_bytes=0  (months 4-7 will be stored)")
print(f"  VirtualChunkContainer A: {source_prefix_a}/")
print(f"  VirtualChunkContainer B: {source_prefix_b}/")

# ---------------------------------------------------------------------------
# Helper to open the repo with a specific inline threshold
# ---------------------------------------------------------------------------

def open_repo(inline_threshold: int = 0) -> icechunk.Repository:
    return icechunk.Repository.open(
        icechunk.local_filesystem_storage(str(REPO_DIR)),
        config=make_config(inline_threshold),
        authorize_virtual_chunk_access=credentials,
    )


# ---------------------------------------------------------------------------
# Commit 1: Create the array skeleton + add virtual chunks for months 0-3
# ---------------------------------------------------------------------------
session = repo.writable_session("main")
store = session.store

root = zarr.open_group(store, mode="a", zarr_format=3)

climate = root.require_array(
    name="climate_data",
    shape=(12, 4),
    chunks=(1, 4),
    dtype="<f4",
    compressors=None,
    fill_value=float("nan"),
)
climate.attrs["description"] = (
    "Monthly climate data for 4 stations. "
    "Months 0-1: virtual chunks (source-a), 2-3: virtual chunks (source-b), "
    "4-7: stored chunks, 8-11: inline chunks."
)
climate.attrs["units"] = "Kelvin"

# Register virtual refs for months 0-1 pointing to source-a
for month, fpath in virtual_files_a.items():
    store.set_virtual_ref(
        f"climate_data/c/{month}/0",
        f"file://{fpath}",
        offset=0,
        length=CHUNK_BYTES,
    )

# Register virtual refs for months 2-3 pointing to source-b
for month, fpath in virtual_files_b.items():
    store.set_virtual_ref(
        f"climate_data/c/{month}/0",
        f"file://{fpath}",
        offset=0,
        length=CHUNK_BYTES,
    )

commit1 = session.commit(
    "Add climate_data array skeleton and virtual chunks for months 0-3 (Q1)"
)
print(f"\nCommit 1: {commit1}")
print("  Added: array skeleton + virtual chunk refs for months 0, 1 (source-a) and 2, 3 (source-b)")

# ---------------------------------------------------------------------------
# Commit 2: Add native/stored chunks for months 4-7
# ---------------------------------------------------------------------------
# inline_chunk_threshold_bytes=0 → all written chunks go to stored (not inline)
repo = open_repo(inline_threshold=0)
session = repo.writable_session("main")
store = session.store

root = zarr.open_group(store, mode="a", zarr_format=3)
climate = root["climate_data"]

# Write months 4-7 as normal zarr writes; they land as stored (native) chunks.
native_data = {
    4: np.array([286.7, 287.8, 288.9, 290.0], dtype="<f4"),  # May
    5: np.array([291.1, 292.2, 293.3, 294.4], dtype="<f4"),  # June
    6: np.array([295.5, 296.6, 297.7, 298.8], dtype="<f4"),  # July
    7: np.array([299.9, 300.0, 301.1, 302.2], dtype="<f4"),  # August
}

for month, data in native_data.items():
    climate[month, :] = data

commit2 = session.commit(
    "Add native/stored chunks for months 4-7 (Q2/Q3): 4 new stored chunks"
)
print(f"\nCommit 2: {commit2}")
print("  Added: stored chunk writes for months 4, 5, 6, 7")

# ---------------------------------------------------------------------------
# Commit 3: Add inline chunks for months 8-11
# ---------------------------------------------------------------------------
# inline_chunk_threshold_bytes=20 → 16-byte chunks go inline (16 <= 20)
repo = open_repo(inline_threshold=20)
session = repo.writable_session("main")
store = session.store

root = zarr.open_group(store, mode="a", zarr_format=3)
climate = root["climate_data"]

inline_data = {
    8:  np.array([303.3, 304.4, 305.5, 306.6], dtype="<f4"),  # September
    9:  np.array([307.7, 308.8, 309.9, 310.0], dtype="<f4"),  # October
    10: np.array([311.1, 312.2, 313.3, 314.4], dtype="<f4"),  # November
    11: np.array([315.5, 316.6, 317.7, 318.8], dtype="<f4"),  # December
}

for month, data in inline_data.items():
    climate[month, :] = data

commit3 = session.commit(
    "Add inline chunks for months 8-11 (Q4): 4 new inline chunks (16 bytes each <= 20-byte threshold)"
)
print(f"\nCommit 3: {commit3}")
print("  Added: inline chunk writes for months 8, 9, 10, 11")

# ---------------------------------------------------------------------------
# Verification read-back
# ---------------------------------------------------------------------------
print("\n--- Verification read-back ---")
repo = open_repo(inline_threshold=20)
session = repo.readonly_session("main")
store = session.store

root = zarr.open_group(store, mode="r", zarr_format=3)
climate = root["climate_data"]

all_data = climate[:]

expected_rows = {**virtual_source_data, **native_data, **inline_data}

ok = True
for month in range(12):
    expected = expected_rows[month]
    actual = all_data[month]
    match = np.allclose(actual, expected)
    marker = "OK" if match else "MISMATCH"
    if not match:
        ok = False
    print(f"  month {month:2d}: {actual.tolist()}  [{marker}]")

if ok:
    print("  All 12 months match expected values.")
else:
    print("  WARNING: Some months did not match!")

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
print("\n=== Summary ===")
print(f"\nArray: /climate_data  shape=(12, 4)  chunks=(1, 4)  dtype=float32")
print(f"Chunk size: {CHUNK_BYTES} bytes each")
print()
print(f"{'Month':>6}  {'Chunk index':>11}  {'Type':>8}  {'Description'}")
print(f"{'------':>6}  {'-----------':>11}  {'--------':>8}  {'-----------'}")
chunk_types = (
    [(m, "virtual",  f"source-a: {virtual_files_a[m].name}")  for m in range(0, 2)] +
    [(m, "virtual",  f"source-b: {virtual_files_b[m].name}")  for m in range(2, 4)] +
    [(m, "stored",   "native icechunk chunk")                  for m in range(4, 8)] +
    [(m, "inline",   "embedded in snapshot manifest")          for m in range(8, 12)]
)
for month, ctype, desc in chunk_types:
    print(f"  {month:4d}    c/{month}/0       {ctype:>8}  {desc}")

print()
print(f"Inline threshold: 20 bytes (chunk size 16 bytes → months 8-11 inline)")
print(f"Stored threshold: 0 bytes  (chunk size 16 bytes → months 4-7 stored)")
print()
print(f"Virtual source A (obs station A, months 0-1): {SOURCE_DIR_A}")
print(f"Virtual source B (obs station B, months 2-3): {SOURCE_DIR_B}")
print(f"Icechunk repo: {REPO_DIR}")
print()
print(f"Commits:")
print(f"  1: {commit1}  (virtual chunks, months 0-3)")
print(f"  2: {commit2}  (stored chunks,  months 4-7)")
print(f"  3: {commit3}  (inline chunks,  months 8-11)")
print()
print("To explore with core-drill:")
print(f"  cargo run -- {REPO_DIR}")
