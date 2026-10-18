#!/bin/bash
# CocoaPods build phase (see Checkgate.podspec): compiles the bundled Rust
# evaluation engine for the SDK and architectures Xcode is building, and leaves
# libcheckgate_rn.a in this pod's build directory, where the linker finds it.
# Needs rustup with the iOS targets: aarch64-apple-ios, aarch64-apple-ios-sim
# and x86_64-apple-ios.
set -euo pipefail

CRATE_DIR="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"

targets=()
for arch in $ARCHS; do
  case "$PLATFORM_NAME:$arch" in
    iphoneos:arm64) targets+=(aarch64-apple-ios) ;;
    iphonesimulator:arm64) targets+=(aarch64-apple-ios-sim) ;;
    iphonesimulator:x86_64) targets+=(x86_64-apple-ios) ;;
    *)
      echo "error: Checkgate: unsupported platform/architecture $PLATFORM_NAME/$arch" >&2
      exit 1
      ;;
  esac
done

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: Checkgate: cargo not found. Install Rust (https://rustup.rs) and run: rustup target add ${targets[*]}" >&2
  exit 1
fi

RUST_TARGET_DIR="${TARGET_TEMP_DIR}/checkgate-rust"
libs=()
for target in "${targets[@]}"; do
  # Xcode's SDKROOT points at the iOS SDK, which breaks the host build scripts
  # of Rust dependencies; rustc finds the iOS SDK through xcrun by itself.
  env -u SDKROOT cargo rustc \
    --manifest-path "$CRATE_DIR/Cargo.toml" \
    --lib --release --crate-type staticlib \
    --target "$target" \
    --target-dir "$RUST_TARGET_DIR"
  libs+=("$RUST_TARGET_DIR/$target/release/libcheckgate_rn.a")
done

mkdir -p "$CONFIGURATION_BUILD_DIR"
lipo -create "${libs[@]}" -output "$CONFIGURATION_BUILD_DIR/libcheckgate_rn.a"
