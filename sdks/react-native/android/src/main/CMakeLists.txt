cmake_minimum_required(VERSION 3.13)

project(Checkgate)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# JSI headers and libjsi.so come from react-android's prefab package
# (`buildFeatures { prefab true }` in build.gradle).
find_package(ReactAndroid REQUIRED CONFIG)

# The Rust evaluation engine, built per ABI into CHECKGATE_RUST_TARGET_DIR by
# the `buildCheckgateRust` Gradle task (cargo ndk) before this project builds.
if(ANDROID_ABI STREQUAL "arm64-v8a")
  set(CHECKGATE_RUST_TRIPLE aarch64-linux-android)
elseif(ANDROID_ABI STREQUAL "armeabi-v7a")
  set(CHECKGATE_RUST_TRIPLE armv7-linux-androideabi)
elseif(ANDROID_ABI STREQUAL "x86")
  set(CHECKGATE_RUST_TRIPLE i686-linux-android)
elseif(ANDROID_ABI STREQUAL "x86_64")
  set(CHECKGATE_RUST_TRIPLE x86_64-linux-android)
else()
  message(FATAL_ERROR "Checkgate: unsupported Android ABI ${ANDROID_ABI}")
endif()

set(CHECKGATE_RUST_LIB
    "${CHECKGATE_RUST_TARGET_DIR}/${CHECKGATE_RUST_TRIPLE}/release/libcheckgate_rn.a")
add_library(checkgate_rust STATIC IMPORTED)
set_target_properties(checkgate_rust PROPERTIES IMPORTED_LOCATION "${CHECKGATE_RUST_LIB}")

add_library(
    checkgate
    SHARED
    ../../../cpp/checkgate_jsi.cpp
    cpp/checkgate-adapter.cpp
)

target_include_directories(checkgate PRIVATE ../../../cpp)

target_link_libraries(
    checkgate
    ReactAndroid::jsi
    checkgate_rust
    android
    log
    dl
    m
)
