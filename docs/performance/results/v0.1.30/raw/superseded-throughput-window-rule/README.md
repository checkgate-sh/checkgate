# Superseded: throughput runs of 2026-11-11 07:08–07:24 UTC

Three complete, error-free runs, replaced after a harness fix and kept as evidence.

`server/examples/throughput.rs` counted a request only when it both started and
finished inside the 60 s window. In a closed loop, that drops requests at both edges
and undercounts req/s by about mean latency / window: ~0.03 % at 10 clients, but
~3.5 % at 1,000 clients (2.1 s / 60 s). The harness now counts completions inside the
window, and the throughput phase was re-run with the fix. Nothing else changed.

Median req/s across these runs, for comparison: 506 / 500 / 446 / 431 at
10 / 100 / 500 / 1,000 clients.
