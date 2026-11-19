# Superseded: polling profiles, first run (2026-11-18 17:57–18:45 UTC)

Complete, error-free runs except 10,000 clients at 10 s (not sustained, as in
the replacement). Replaced because the harness's **staleness** figure was
wrong: each poll that returned flag value *v* produced one sample for *v*
only, so changes that a newer one replaced before the client polled were never
counted. The samples were therefore at most about one update gap (2 s) old,
except for the last change: p50 came out 1.2–1.4 s at every interval, where a
60 s interval should give about 30 s.

Fixed in `bench/public/sdk/connections.mjs`: a poll that returns change *k*
after the client last saw change *j* counts as the first sight of every change
*j+1…k*, each timed from its own PATCH. Every polling profile was then run
again on a fresh stack (`COST_MODES=poll bench/public/run.sh cost`). The other
figures in these files (requests, CPU, memory, bytes) are kept for reference
and were not used. Their entries in `runs.jsonl` carry `"superseded"`.
