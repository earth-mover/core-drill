# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "icechunk>=2.0.0",
#     "numpy",
#     "zarr>=3",
# ]
# ///
"""Create a test Icechunk repo with adversarial names to stress-test sanitization.

Groups and arrays have names containing ANSI escape codes, terminal control
sequences, null bytes, unicode tricks, and other payloads that could break
a naive terminal renderer.

Usage:
    uv run scripts/create_adversarial_repo.py
"""

import shutil
from pathlib import Path

import numpy as np

REPO_PATH = Path(__file__).parent.parent / "test-data" / "adversarial-repo"

# Adversarial payloads — each is (group_or_array_name, description)
ADVERSARIAL_GROUPS = [
    # ANSI color codes
    ("\x1b[31mred_group\x1b[0m", "ANSI red color"),
    # Cursor movement
    ("\x1b[2J\x1b[Hcleared", "ANSI clear screen + home"),
    # OSC title change
    ("\x1b]0;evil_title\x07group", "OSC set terminal title"),
    # Null bytes
    ("null\x00byte", "embedded null byte"),
    # Bell character
    ("bell\x07ring", "bell character"),
    # Carriage return (overwrite line)
    ("visible\rsecret", "carriage return overwrite"),
    # Backspace overwrite
    ("normal\x08\x08\x08\x08\x08\x08evil!!", "backspace overwrite"),
    # Unicode RTL override
    ("data_\u202emanifest.exe", "RTL override"),
    # Zero-width joiner / spaces
    ("look\u200bnormal", "zero-width space"),
    # Extremely long name
    ("A" * 500, "500-char name"),
    # Newlines in name
    ("line1\nline2", "embedded newline"),
    # Tab characters
    ("col1\tcol2\tcol3", "embedded tabs"),
    # CSI sequences (cursor movement)
    ("\x1b[10;10Hplaced", "CSI cursor positioning"),
    # DCS sequence
    ("\x1bPdevice_control\x1b\\group", "DCS device control"),
    # Clean group for comparison
    ("clean_group", "normal safe name"),
]

ADVERSARIAL_COMMIT_MESSAGES = [
    "normal commit",
    "\x1b[31mRED ALERT\x1b[0m data corruption",
    "\x1b]0;pwned\x07 updated arrays",
    "line1\nline2\nline3 multiline commit",
    "contains \x00 null byte",
    "has \x07 bell char",
    "overwrite\rsneaky commit message",
    "\x1b[2J\x1b[H cleared your screen lol",
    "RTL trick: \u202egnp.elif",
    "long " + "x" * 300 + " commit",
]


def main():
    import icechunk
    import zarr

    # Clean slate
    if REPO_PATH.exists():
        shutil.rmtree(REPO_PATH)

    repo = icechunk.Repository.create(
        icechunk.local_filesystem_storage(str(REPO_PATH)),
    )

    session = repo.writable_session("main")
    store = session.store

    root = zarr.open_group(store, mode="w")

    # Create groups with adversarial names
    for name, desc in ADVERSARIAL_GROUPS:
        try:
            grp = root.create_group(name)
            # Add a child array so the group isn't empty
            grp.create_array(
                "data",
                shape=(4, 4),
                chunks=(2, 2),
                dtype="float32",
                fill_value=0.0,
            )
        except Exception as e:
            print(f"  SKIP group '{desc}': {e}")

    # Create arrays at root level with adversarial names
    adversarial_arrays = [
        ("\x1b[31mred_array\x1b[0m", "ANSI colored array"),
        ("\x1b]2;evil\x07temps", "OSC title in array name"),
        ("normal_array", "safe array name"),
    ]
    for name, desc in adversarial_arrays:
        try:
            arr = root.create_array(
                name,
                shape=(10,),
                chunks=(5,),
                dtype="int32",
                fill_value=-1,
            )
            arr[:] = np.arange(10, dtype="int32")
        except Exception as e:
            print(f"  SKIP array '{desc}': {e}")

    # First commit
    session.commit("initial commit with adversarial names")
    print("Committed: initial adversarial data")

    # Make additional commits with adversarial messages
    for i, msg in enumerate(ADVERSARIAL_COMMIT_MESSAGES):
        session = repo.writable_session("main")
        store = session.store
        root = zarr.open_group(store, mode="r+")
        try:
            arr = root.create_array(
                f"commit_{i}",
                shape=(2,),
                chunks=(2,),
                dtype="int32",
                fill_value=0,
            )
            arr[:] = np.array([i, i + 1], dtype="int32")
            session.commit(msg)
            print(f"Committed: {repr(msg)[:60]}")
        except Exception as e:
            print(f"  SKIP commit {i}: {e}")

    # Create branches with adversarial names
    adversarial_branches = [
        "\x1b[31mred_branch\x1b[0m",
        "normal_branch",
        "\x1b]0;evil\x07branch",
        "has\nnewline",
    ]
    for branch_name in adversarial_branches:
        try:
            repo.create_branch(branch_name, repo.lookup_branch("main"))
            print(f"Created branch: {repr(branch_name)[:60]}")
        except Exception as e:
            print(f"  SKIP branch {repr(branch_name)[:40]}: {e}")

    # Create tags with adversarial names
    adversarial_tags = [
        "\x1b[32mgreen_tag\x1b[0m",
        "v1.0",
        "\x1b[2Jcleared_tag",
    ]
    for tag_name in adversarial_tags:
        try:
            repo.create_tag(tag_name, repo.lookup_branch("main"))
            print(f"Created tag: {repr(tag_name)[:60]}")
        except Exception as e:
            print(f"  SKIP tag {repr(tag_name)[:40]}: {e}")

    print(f"\nAdversarial repo created at: {REPO_PATH}")


if __name__ == "__main__":
    main()
