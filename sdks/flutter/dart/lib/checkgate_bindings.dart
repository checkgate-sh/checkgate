// ignore_for_file: non_constant_identifier_names, camel_case_types
/// Raw Dart FFI bindings to libcheckgate_flutter.
///
/// These typedefs mirror the C signatures in sdks/flutter/src/lib.rs exactly.
/// Do not call these directly — use [CheckgateFlutterClient] instead.
library checkgate_bindings;

import 'dart:ffi';
import 'dart:io';
import 'package:ffi/ffi.dart';

// ---------------------------------------------------------------------------
// Native (C ABI) function signatures
// ---------------------------------------------------------------------------

typedef _UpsertFlagNative = Void Function(
  Pointer<Utf8> key,
  Bool isEnabled,
  Int32 rolloutPercentage,
  Pointer<Utf8> rulesJson,
);

typedef _UpsertFlagV2Native = Void Function(Pointer<Utf8> flagJson);

typedef _DeleteFlagNative = Void Function(Pointer<Utf8> key);

typedef _ClearStoreNative = Void Function();

typedef _IsEnabledNative = Int32 Function(
  Pointer<Utf8> flagKey,
  Pointer<Utf8> userKey,
  Pointer<Utf8> attributesJson,
);

typedef _GetVariantNative = Pointer<Utf8> Function(
  Pointer<Utf8> flagKey,
  Pointer<Utf8> userKey,
  Pointer<Utf8> attributesJson,
);

typedef _FreeStringNative = Void Function(Pointer<Utf8> s);

// ---------------------------------------------------------------------------
// Dart function types (used by lookupFunction)
// ---------------------------------------------------------------------------

typedef UpsertFlagFn = void Function(
  Pointer<Utf8> key,
  bool isEnabled,
  int rolloutPercentage,
  Pointer<Utf8> rulesJson,
);

typedef UpsertFlagV2Fn = void Function(Pointer<Utf8> flagJson);

typedef DeleteFlagFn = void Function(Pointer<Utf8> key);

typedef ClearStoreFn = void Function();

typedef IsEnabledFn = int Function(
  Pointer<Utf8> flagKey,
  Pointer<Utf8> userKey,
  Pointer<Utf8> attributesJson,
);

typedef GetVariantFn = Pointer<Utf8> Function(
  Pointer<Utf8> flagKey,
  Pointer<Utf8> userKey,
  Pointer<Utf8> attributesJson,
);

typedef FreeStringFn = void Function(Pointer<Utf8> s);

// ---------------------------------------------------------------------------
// Binding class — loads symbols from the compiled Rust library.
// ---------------------------------------------------------------------------

class CheckgateBindings {
  final UpsertFlagFn checkgate_upsert_flag;
  final UpsertFlagV2Fn checkgate_upsert_flag_v2;
  final DeleteFlagFn checkgate_delete_flag;
  final ClearStoreFn checkgate_clear_store;
  final IsEnabledFn checkgate_is_enabled;
  final GetVariantFn checkgate_get_variant;
  final FreeStringFn checkgate_free_string;

  CheckgateBindings(DynamicLibrary lib)
      : checkgate_upsert_flag = lib.lookupFunction<_UpsertFlagNative, UpsertFlagFn>(
            'checkgate_upsert_flag'),
        checkgate_upsert_flag_v2 = lib.lookupFunction<_UpsertFlagV2Native, UpsertFlagV2Fn>(
            'checkgate_upsert_flag_v2'),
        checkgate_delete_flag = lib.lookupFunction<_DeleteFlagNative, DeleteFlagFn>(
            'checkgate_delete_flag'),
        checkgate_clear_store = lib.lookupFunction<_ClearStoreNative, ClearStoreFn>(
            'checkgate_clear_store'),
        checkgate_is_enabled = lib.lookupFunction<_IsEnabledNative, IsEnabledFn>(
            'checkgate_is_enabled'),
        checkgate_get_variant = lib.lookupFunction<_GetVariantNative, GetVariantFn>(
            'checkgate_get_variant'),
        checkgate_free_string = lib.lookupFunction<_FreeStringNative, FreeStringFn>(
            'checkgate_free_string');

  /// Opens the correct shared library for the current platform.
  factory CheckgateBindings.open() {
    final DynamicLibrary lib;

    if (Platform.isIOS || Platform.isMacOS) {
      // iOS / macOS: static link — symbols are already in the process image.
      lib = DynamicLibrary.process();
    } else if (Platform.isAndroid) {
      lib = DynamicLibrary.open('libcheckgate_flutter.so');
    } else if (Platform.isLinux) {
      lib = DynamicLibrary.open('libcheckgate_flutter.so');
    } else if (Platform.isWindows) {
      lib = DynamicLibrary.open('checkgate_flutter.dll');
    } else {
      throw UnsupportedError(
          'CheckgateBindings: unsupported platform ${Platform.operatingSystem}');
    }

    return CheckgateBindings(lib);
  }
}
