#!/bin/sh
# Build helper for meson → cargo integration

export MESON_BUILD_ROOT="$1"
export MESON_SOURCE_ROOT="$2"
export CARGO_TARGET_DIR="$MESON_BUILD_ROOT/src"
export CARGO_HOME="${CARGO_HOME:-$MESON_BUILD_ROOT/cargo-home}"
OUTPUT="$3"
BUILDTYPE="$4"
APP_NAME="$5"

if [ "$BUILDTYPE" = "release" ]; then
    echo "CARGO: release build"
    cargo build --manifest-path "$MESON_SOURCE_ROOT/Cargo.toml" --release && \
        cp "$CARGO_TARGET_DIR/release/$APP_NAME" "$OUTPUT"
else
    echo "CARGO: debug build"
    cargo build --manifest-path "$MESON_SOURCE_ROOT/Cargo.toml" && \
        cp "$CARGO_TARGET_DIR/debug/$APP_NAME" "$OUTPUT"
fi
