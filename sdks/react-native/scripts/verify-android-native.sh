#!/bin/bash
# Checks that the npm package of @checkgate/react-native produces a working
# Android native library, without needing a React Native app:
#   1. bundle + `npm pack` (in a copy — bundle-rust.js edits files in place),
#   2. build the Rust engine from the unpacked tarball with cargo ndk, exactly
#      as android/build.gradle does (outside the monorepo workspace),
#   3. compile the JSI host + JNI adapter against React Native's JSI and link
#      libcheckgate.so with --no-undefined, then check the JNI entry point.
# Needs: node/npm, cargo with cargo-ndk and the Android targets, and an NDK
# (ANDROID_NDK_HOME, or ANDROID_NDK_LATEST_HOME on GitHub runners).
set -euo pipefail

RN_VERSION="${RN_VERSION:-0.76.9}"
ABIS="${ABIS:-arm64-v8a x86_64}"
SDK_DIR="$(cd "$(dirname "$0")/.." && pwd)"
REPO_DIR="$(cd "$SDK_DIR/../.." && pwd)"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:-}}"
[ -n "$NDK" ] || { echo "Set ANDROID_NDK_HOME" >&2; exit 1; }
export ANDROID_NDK_HOME="$NDK"
CLANG="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/clang++"
NM="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-nm"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "== Packing"
mkdir -p "$WORK/repo/sdks"
cp "$REPO_DIR/Cargo.toml" "$WORK/repo/"
cp -r "$REPO_DIR/core" "$WORK/repo/core"
tar -C "$REPO_DIR/sdks" --exclude=node_modules -cf - react-native | tar -C "$WORK/repo/sdks" -xf -
(cd "$WORK/repo/sdks/react-native" && node scripts/bundle-rust.js && npm pack --silent --pack-destination "$WORK")
mkdir -p "$WORK/pkg"
tar -xzf "$WORK"/checkgate-react-native-*.tgz -C "$WORK/pkg"
PKG="$WORK/pkg/package"

echo "== React Native $RN_VERSION JSI headers and libjsi"
(cd "$WORK" && npm pack --silent "react-native@$RN_VERSION" >/dev/null && tar -xzf "react-native-$RN_VERSION.tgz" package/ReactCommon/jsi >/dev/null)
curl -sSfL -o "$WORK/react-android.aar" \
  "https://repo1.maven.org/maven2/com/facebook/react/react-android/$RN_VERSION/react-android-$RN_VERSION-release.aar"
(cd "$WORK" && unzip -q react-android.aar 'prefab/modules/jsi/*')
JSI="$WORK/prefab/modules/jsi"

for abi in $ABIS; do
  case "$abi" in
    arm64-v8a) triple=aarch64-linux-android ;;
    armeabi-v7a) triple=armv7-linux-androideabi; clang_triple=armv7a-linux-androideabi ;;
    x86) triple=i686-linux-android ;;
    x86_64) triple=x86_64-linux-android ;;
    *) echo "unknown ABI $abi" >&2; exit 1 ;;
  esac
  echo "== $abi"
  (cd "$PKG" && cargo ndk --platform 23 -t "$abi" build --release --lib --target-dir "$WORK/rust")
  "$CLANG" --target="${clang_triple:-$triple}23" -std=c++20 -O2 -frtti -fexceptions -fPIC -shared \
    -Wl,--no-undefined \
    -I"$PKG/cpp" -I"$JSI/include" \
    "$PKG/cpp/checkgate_jsi.cpp" "$PKG/android/src/main/cpp/checkgate-adapter.cpp" \
    "$WORK/rust/$triple/release/libcheckgate_rn.a" "$JSI/libs/android.$abi/libjsi.so" \
    -landroid -llog -ldl -lm \
    -o "$WORK/libcheckgate-$abi.so"
  # Not `grep -q`: closing the pipe early makes llvm-nm fail under pipefail.
  "$NM" -D --defined-only "$WORK/libcheckgate-$abi.so" \
    | grep Java_com_checkgate_reactnative_CheckgateModule_nativeInstall >/dev/null
  unset clang_triple
done
echo "Android native library links for: $ABIS"
