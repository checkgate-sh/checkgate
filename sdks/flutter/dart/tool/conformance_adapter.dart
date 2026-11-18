// Drives one CheckgateClient for the shared fault-injection suite
// (sdks/conformance/flutter.mjs runs this and sends it the scenarios). One
// JSON object per line on stdin, one reply per line on stdout:
//
//   {"id":1,"cmd":"create","url":"…","key":"…","timing":{…}}
//   {"id":2,"cmd":"connect"}  → {"id":2,"ok":true}  or  {"id":2,"ok":false,"error":"…"}
//   {"id":3,"cmd":"flags"}    → {"id":3,"flags":{key: flag}}
//   {"id":4,"cmd":"evaluate","key":"…","user":"…"} → {"id":4,"value":true|false|null}
//   {"id":5,"cmd":"status"}   → {"id":5,"state":"…"}
//   {"id":7,"cmd":"sample","users":[…]} → flags and evaluations at one instant
//   {"id":6,"cmd":"close"}
//
// With CHECKGATE_FLUTTER_LIB=<path to libflutter.{dylib,so}> the real Rust
// engine evaluates; otherwise flags are tracked without an engine and
// `evaluate` answers null.
import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import '../checkgate_bindings.dart';
import '../checkgate_flutter.dart';

const _states = {
  CheckgateConnectionState.idle: 'idle',
  CheckgateConnectionState.connecting: 'connecting',
  CheckgateConnectionState.streaming: 'streaming',
  CheckgateConnectionState.reconnecting: 'reconnecting',
  CheckgateConnectionState.polling: 'polling',
  CheckgateConnectionState.authFailed: 'auth_failed',
  CheckgateConnectionState.closed: 'closed',
};

Future<void> main() async {
  final libPath = Platform.environment['CHECKGATE_FLUTTER_LIB'];
  final engine = libPath != null && libPath.isNotEmpty;
  CheckgateClient? client;
  void reply(Map<String, Object?> msg) => stdout.writeln(jsonEncode(msg));

  await for (final line
      in stdin.transform(utf8.decoder).transform(const LineSplitter())) {
    final msg = jsonDecode(line) as Map<String, dynamic>;
    final id = msg['id'];
    switch (msg['cmd']) {
      case 'create':
        final t = msg['timing'] as Map<String, dynamic>;
        Duration ms(String k) => Duration(milliseconds: (t[k] as num).round());
        client = CheckgateClient(
          serverUrl: msg['url'] as String,
          sdkKey: msg['key'] as String,
          reportImpressions: false,
          reconnectBaseDelay: ms('reconnectDelayMs'),
          maxReconnectDelay: ms('maxReconnectDelayMs'),
          pollFallbackThreshold: t['pollFallbackThreshold'] as int,
          pollInterval: ms('pollIntervalMs'),
          connectTimeout: ms('connectTimeoutMs'),
          bindings: engine ? CheckgateBindings(DynamicLibrary.open(libPath)) : null,
        );
        reply({'id': id, 'ok': true, 'engine': engine});
      case 'connect':
        final c = client!;
        (engine ? c.connect() : c.debugConnectWithoutEngine()).then(
          (_) => reply({'id': id, 'ok': true}),
          onError: (Object e) => reply({'id': id, 'ok': false, 'error': '$e'}),
        );
      case 'flags':
        reply({'id': id, 'flags': client!.debugFlags});
      case 'evaluate':
        reply({
          'id': id,
          'value': engine
              ? client!.isEnabled(msg['key'] as String, msg['user'] as String,
                  {'plan': 'pro'})
              : null,
        });
      case 'sample':
        // Flags and evaluations at one instant: no event is handled between.
        final c = client!;
        final flags = Map<String, Object?>.of(c.debugFlags);
        final evals = <String, Object?>{};
        for (final key in flags.keys) {
          for (final user in (msg['users'] as List).cast<String>()) {
            evals['$key/$user'] =
                engine ? c.isEnabled(key, user, {'plan': 'pro'}) : null;
          }
        }
        reply({'id': id, 'flags': flags, 'evals': evals});
      case 'status':
        reply({'id': id, 'state': _states[client!.status.state]});
      case 'close':
        client?.close();
        reply({'id': id, 'ok': true});
      case 'exit':
        reply({'id': id, 'ok': true});
        exit(0);
    }
  }
}
