# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "icechunk",
#   "zarr",
#   "numpy",
# ]
# ///
"""
Create a test Icechunk repository with virtual chunk references pointing to local files.

Source files go to:    test-data/source-files/
Icechunk repo goes to: test-data/virtual-chunks-repo/
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
SOURCE_DIR = ROOT / "test-data" / "source-files"
REPO_DIR = ROOT / "test-data" / "virtual-chunks-repo"

SOURCE_DIR.mkdir(parents=True, exist_ok=True)
REPO_DIR.mkdir(parents=True, exist_ok=True)


# ---------------------------------------------------------------------------
# Step 1 – Create small binary source files
# ---------------------------------------------------------------------------
# Each file is a compact binary blob: a 4-byte little-endian header
# (number of float32 values) followed by the raw float32 data.  This
# intentionally looks "not like zarr" so the virtual refs are meaningful.

def write_raw_f32(path: Path, data: np.ndarray) -> None:
    """Write a header + raw float32 array to *path*."""
    flat = data.astype("<f4").tobytes()
    header = struct.pack("<I", len(data.flat))
    path.write_bytes(header + flat)
    print(f"  wrote {path}  ({len(data.flat)} float32 values, {path.stat().st_size} bytes)")


HEADER_BYTES = 4  # struct "<I"
FLOAT32_BYTES = 4

# File 1: temperature – shape (4,), values 270..273 K
temperature_data = np.array([270.0, 271.5, 272.3, 273.1], dtype="<f4")
temperature_file = SOURCE_DIR / "temperature.bin"
write_raw_f32(temperature_file, temperature_data)

# File 2: salinity – shape (4,), values 34..35 PSU
salinity_data = np.array([34.1, 34.5, 34.8, 35.0], dtype="<f4")
salinity_file = SOURCE_DIR / "salinity.bin"
write_raw_f32(salinity_file, salinity_data)

# File 3: pressure – shape (2, 4), values ~1013 hPa
pressure_data = np.array([[1012.0, 1013.0, 1014.0, 1015.0],
                           [1011.5, 1012.5, 1013.5, 1014.5]], dtype="<f4")
pressure_file = SOURCE_DIR / "pressure.bin"
write_raw_f32(pressure_file, pressure_data)

print()

# ---------------------------------------------------------------------------
# Step 2 – Create the Icechunk repository
# ---------------------------------------------------------------------------
# We need one VirtualChunkContainer per path prefix that contains our files.
# Since all source files live under SOURCE_DIR we register that prefix.

source_prefix = f"file://{SOURCE_DIR}"

config = icechunk.RepositoryConfig.default()
config.set_virtual_chunk_container(
    icechunk.VirtualChunkContainer(
        source_prefix + "/",
        icechunk.local_filesystem_store(str(SOURCE_DIR)),
    )
)

# authorize_virtual_chunk_access maps each prefix to None (no credentials
# needed for local files).
credentials = icechunk.containers_credentials({source_prefix + "/": None})

storage = icechunk.local_filesystem_storage(str(REPO_DIR))

repo = icechunk.Repository.create(
    storage,
    config=config,
    authorize_virtual_chunk_access=credentials,
)
print(f"Created repo at {REPO_DIR}")


# ---------------------------------------------------------------------------
# Helper to reopen the repo with virtual chunk access authorised
# ---------------------------------------------------------------------------
def open_repo() -> icechunk.Repository:
    return icechunk.Repository.open(
        icechunk.local_filesystem_storage(str(REPO_DIR)),
        config=config,
        authorize_virtual_chunk_access=credentials,
    )


# ---------------------------------------------------------------------------
# Step 3 – Commit 1: add temperature and salinity arrays with virtual refs
# ---------------------------------------------------------------------------
session = repo.writable_session("main")
store = session.store

root = zarr.open_group(store, mode='a', zarr_format=3)

# --- temperature (shape 4, one chunk of 4) ---
temp_arr = root.require_array(
    name="temperature",
    shape=(4,),
    chunks=(4,),
    dtype="<f4",
    compressors=None,
    fill_value=float("nan"),
)
# The single chunk (index 0) lives at byte offset HEADER_BYTES in the file.
store.set_virtual_ref(
    "temperature/c/0",
    f"file://{temperature_file}",
    offset=HEADER_BYTES,
    length=4 * FLOAT32_BYTES,
)

# --- salinity (shape 4, one chunk of 4) ---
sal_arr = root.require_array(
    name="salinity",
    shape=(4,),
    chunks=(4,),
    dtype="<f4",
    compressors=None,
    fill_value=float("nan"),
)
store.set_virtual_ref(
    "salinity/c/0",
    f"file://{salinity_file}",
    offset=HEADER_BYTES,
    length=4 * FLOAT32_BYTES,
)

commit1 = session.commit("Add temperature and salinity arrays with virtual refs")
print(f"Commit 1: {commit1}")

# ---------------------------------------------------------------------------
# Step 4 – Commit 2: add pressure array (2-D, two chunks of 4 each)
# ---------------------------------------------------------------------------
repo = open_repo()
session = repo.writable_session("main")
store = session.store

root = zarr.open_group(store, mode='a', zarr_format=3)

# pressure: shape (2, 4), chunks (1, 4) → two chunks along axis-0
pres_arr = root.require_array(
    name="pressure",
    shape=(2, 4),
    chunks=(1, 4),
    dtype="<f4",
    compressors=None,
    fill_value=float("nan"),
)

row_bytes = 4 * FLOAT32_BYTES
# chunk [0, 0] → first row of pressure.bin
store.set_virtual_ref(
    "pressure/c/0/0",
    f"file://{pressure_file}",
    offset=HEADER_BYTES,
    length=row_bytes,
)
# chunk [1, 0] → second row of pressure.bin
store.set_virtual_ref(
    "pressure/c/1/0",
    f"file://{pressure_file}",
    offset=HEADER_BYTES + row_bytes,
    length=row_bytes,
)

commit2 = session.commit("Add 2-D pressure array with two virtual chunk refs")
print(f"Commit 2: {commit2}")

# ---------------------------------------------------------------------------
# Step 5 – Commit 3: add zarr metadata attribute to document provenance
# ---------------------------------------------------------------------------
repo = open_repo()
session = repo.writable_session("main")
store = session.store

root = zarr.open_group(store, mode='a', zarr_format=3)
root.attrs["description"] = (
    "Test virtual-chunk repo: arrays backed by raw binary files in test-data/source-files/"
)
root.attrs["source_dir"] = str(SOURCE_DIR)

commit3 = session.commit("Add root-group metadata describing data provenance")
print(f"Commit 3: {commit3}")

# ---------------------------------------------------------------------------
# Step 6 – Verify by reading back
# ---------------------------------------------------------------------------
print("\nVerification read-back:")
repo = open_repo()
session = repo.readonly_session("main")
store = session.store

root = zarr.open_group(store, mode='r', zarr_format=3)

temp_read = root["temperature"][:]
sal_read  = root["salinity"][:]
pres_read = root["pressure"][:]

print(f"  temperature : {temp_read}")
print(f"  salinity    : {sal_read}")
print(f"  pressure    :\n{pres_read}")

assert np.allclose(temp_read, temperature_data), "temperature mismatch"
assert np.allclose(sal_read,  salinity_data),    "salinity mismatch"
assert np.allclose(pres_read, pressure_data),    "pressure mismatch"
print("  All values match source data - virtual refs working correctly.")

# ---------------------------------------------------------------------------
# Step 7 – Print summary
# ---------------------------------------------------------------------------
print("\n=== Summary ===")
print(f"Source files:")
for f in sorted(SOURCE_DIR.iterdir()):
    print(f"  {f}  ({f.stat().st_size} bytes)")

print(f"\nIcechunk repo: {REPO_DIR}")
print(f"Commits:")
print(f"  1: {commit1}")
print(f"  2: {commit2}")
print(f"  3: {commit3}")
print(f"\nArrays in repo:")
print(f"  /temperature  shape=(4,)    chunks=(4,)    1 virtual chunk -> temperature.bin")
print(f"  /salinity     shape=(4,)    chunks=(4,)    1 virtual chunk -> salinity.bin")
print(f"  /pressure     shape=(2, 4)  chunks=(1, 4)  2 virtual chunks -> pressure.bin")
print(f"\nTo test with cargo:")
print(f"  cargo run -- test-data/virtual-chunks-repo")
