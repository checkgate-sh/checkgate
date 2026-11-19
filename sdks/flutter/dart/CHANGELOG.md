## 0.1.31 (2026-11-19)

- The library is in `lib/`, so `import 'package:checkgate_flutter/checkgate_flutter.dart'`
  resolves. Earlier releases shipped it at the package root, where no app could
  import it.
- A stream that receives no complete frame (event or keep-alive) for 3 × the
  server's announced heartbeat (45 s by default) is treated as dead and
  reconnected; a response that takes longer than `connectTimeout` (30 s) too.
- HTTP 401/403 from the stream or a poll is terminal: retries stop, `connect()`
  completes with a `CheckgateAuthException` (status only), flags already loaded
  are kept, and `connect()` retries.
- `status` reports the connection state, the last confirmed sync and the error.
- `close()` completes a pending `connect()` with a `StateError`, and ends the
  previous SSE loop even if `connect()` follows immediately.
- A delayed stream replay no longer rolls flags back: replays and polls are
  merged per flag by revision, so the newer version wins. Needs a 0.1.31
  server (`/flags/snapshot?v=2`); against an older one the SDK behaves as
  0.1.30 did.
- Large bootstraps no longer lose flags, and `connect()` no longer hangs: SSE
  lines cut across network reads were dropped, sometimes including `ready`.
- A slow poll can no longer roll back newer stream state after a reconnect:
  polls are single-flight, time out after 30 s, and are discarded once the
  stream takes over.
- One invalid (non-string) attribute is skipped on its own instead of erasing
  every attribute of the user.
- Documented how to build and link the native engine; it is not bundled.

## 0.1.30 (2026-11-11)

- Faster rollout and multivariate evaluation: the shared Rust core hashes
  bucketing keys without allocating. Every user gets the same bucket as
  before. No Dart API changes.

## 0.1.25 (2026-10-19)

- Reconnects keep evaluating the last complete flag set until the server's
  replay is finished; a replay cut short is discarded.
- Change events older than the applied flag revision are ignored.
- Conversions carry `occurred_at` and impressions `evaluated_at`.

## 0.1.24 (2026-09-17)

- Release 0.1.24.

## 0.1.23 (2026-08-02)

- Release 0.1.23.

## 0.1.22 (2026-07-18)

- Release 0.1.22.

## 0.1.21 (2026-07-18)

- Release 0.1.21.

## 0.1.20 (2026-07-18)

- Release 0.1.20.

## 0.1.19 (2026-07-04)

- Release 0.1.19.

## 0.1.18 (2026-07-04)

- Release 0.1.18.

## 0.1.17 (2026-06-14)

- Release 0.1.17.

# Changelog

















## 0.1.16

- Release 0.1.16.

## 0.1.15

- Release 0.1.15.

## 0.1.14

- Release 0.1.14.

## 0.1.13

- Release 0.1.13.

## 0.1.12

- Release 0.1.12.

## 0.1.11

- Release 0.1.11.

## 0.1.10

- Release 0.1.10.

## 0.1.9

- Release 0.1.9.

## 0.1.8

- Release 0.1.8.

## 0.1.7

- Release 0.1.7.

## 0.1.6

- Release 0.1.6.

## 0.1.5

- Release 0.1.5.

## 0.1.4

- Release 0.1.4.

## 0.1.3

- Release 0.1.3.

## 0.1.2

- Release 0.1.2.

## 0.1.1

- Release 0.1.1.

## 0.1.0

- Initial release.
- Local flag evaluation via Rust FFI (sub-microsecond, no network round-trip).
- Real-time flag sync via SSE stream.
- `isEnabled` with user key and attribute-based targeting rules.
