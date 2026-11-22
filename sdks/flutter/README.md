<p align="center">
  <img src="../../assets/logo.svg" width="64" alt="Checkgate">
</p>

# Checkgate Flutter SDK

This directory holds the Flutter SDK, published to pub.dev as
[`checkgate_flutter`](https://pub.dev/packages/checkgate_flutter). Flags are
evaluated in-process by the Rust engine through Dart FFI and kept current over
Server-Sent Events.

The package bundles no compiled engine: you build the Rust engine and link it
into your app yourself.

- [`dart/README.md`](dart/README.md): the package README (installation,
  usage, configuration, linking the engine, platform status). It is also the
  pub.dev page, so it is the only copy.
- [`dart/lib/`](dart/lib): the Dart client and FFI bindings.
- [`dart/rust/`](dart/rust): the engine's C ABI, built as `libflutter`.
- [`dart/test/`](dart/test): Dart unit tests (`flutter test`, no engine
  needed).
- Guide: https://checkgate-sh.github.io/checkgate/sdks/flutter
