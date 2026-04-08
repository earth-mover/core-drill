# Default: show available commands
default:
    @just --list

# Run against the public ERA5 test repo on S3
test-s3:
    cargo run -- 's3://icechunk-public-data/v1/era5_weatherbench2'

# Run against a local repo
test-local path:
    cargo run -- '{{path}}'

# Run with debug logging
test-debug:
    RUST_LOG=debug cargo run -- 's3://icechunk-public-data/v1/era5_weatherbench2' 2>debug.log

# Build release
build:
    cargo build --release

# Check + clippy
check:
    cargo check && cargo clippy

# Format
fmt:
    cargo fmt

# Watch for changes and re-check
watch:
    cargo watch -x check
