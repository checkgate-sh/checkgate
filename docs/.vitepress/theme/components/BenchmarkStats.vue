<script setup lang="ts">
// Measured figures from the committed benchmark results (see
// data/benchmarks.data.ts). Every value names its statistic, the version it
// was measured on, and the conditions; nothing here is typed by hand.
import { withBase } from 'vitepress'
import { data as b } from '../../data/benchmarks.data'
import { fmtNs, fmtInt, fmtSig } from '../format'

const evalMedian = fmtNs(b.evaluation.medianNs)
const evalP99 = fmtNs(b.evaluation.p99Ns)
const rust = fmtNs(b.evaluation.rustCoreMedianNs)
const tiles = [
  {
    label: 'Flag evaluation in the Node.js SDK',
    stat: 'median',
    num: evalMedian.num,
    unit: evalMedian.unit,
    detail: `isEnabled(), 50 % rollout, 1,000 flags in memory; p99 ${evalP99.num} ${evalP99.unit}. Rust engine alone: ${rust.num} ${rust.unit}.`,
  },
  {
    label: 'Snapshot throughput, one server',
    stat: 'median of runs',
    num: fmtInt(b.throughput.rps),
    unit: 'req/s',
    detail: `${fmtInt(b.throughput.concurrency)} concurrent clients; ${fmtSig(b.throughput.cpuMsPerRequest)} ms of server CPU per request; peak RSS ${Math.round(b.throughput.rssPeakMib)} MiB.`,
  },
  {
    label: 'Flag change reaching every SDK',
    stat: 'p50',
    num: String(Math.round(b.propagation.p50Ms)),
    unit: 'ms',
    detail: `${fmtInt(b.propagation.clients)} connected SDKs; p99 ${Math.round(b.propagation.p99Ms)} ms pooled over ${b.propagation.runs} runs (median run ${Math.round(b.propagation.runsP99MedianMs)} ms); ${fmtInt(b.propagation.missed)} of ${fmtInt(b.propagation.deliveries)} deliveries missed.`,
  },
]
</script>

<template>
  <div class="cg-bench">
    <ul class="cg-bench-grid">
      <li v-for="t in tiles" :key="t.label" class="cg-bench-tile">
        <p class="cg-bench-label">{{ t.label }}</p>
        <p class="cg-bench-value">
          <span class="num">{{ t.num }}</span> <span class="unit">{{ t.unit }}</span>
          <span class="cg-bench-stat">{{ t.stat }}</span>
        </p>
        <p class="cg-bench-detail">{{ t.detail }}</p>
      </li>
    </ul>
    <p class="cg-bench-foot">
      Measured on v{{ b.version }} ({{ b.date }}, commit <code>{{ b.commit }}</code>) on one {{ b.host }} laptop,
      server limited to {{ b.serverLimits }}, PostgreSQL and Redis in containers. Results describe that machine
      and workload, not a guarantee for yours.
      <a :href="withBase('/performance/benchmarks.html')">Methodology, every run and raw data</a>.
    </p>
  </div>
</template>

<style scoped>
.cg-bench-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 16px;
  margin: 0;
  padding: 0;
  list-style: none;
}
.cg-bench-tile {
  margin: 0 !important;
  padding: 22px 22px 20px;
  border: 1px solid var(--cg-border);
  border-radius: var(--cg-radius);
  background: var(--cg-surface);
}
.cg-bench-label {
  margin: 0 !important;
  font-size: 14px;
  font-weight: 600;
  line-height: 1.4;
  color: var(--cg-ink-2);
}
.cg-bench-value {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 4px 8px;
  margin: 10px 0 8px !important;
  line-height: 1.1;
}
.cg-bench-value .num {
  font-size: 40px;
  font-weight: 700;
  letter-spacing: -0.02em;
  color: var(--cg-ink);
  font-variant-numeric: tabular-nums;
}
.cg-bench-value .unit {
  font-size: 18px;
  font-weight: 600;
  color: var(--cg-ink-2);
}
.cg-bench-stat {
  margin-left: auto;
  align-self: center;
  padding: 2px 9px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 600;
  color: var(--cg-indigo-700);
  background: var(--cg-indigo-50);
}
.dark .cg-bench-stat {
  color: var(--cg-indigo-200);
  background: rgba(99, 102, 241, 0.18);
}
.cg-bench-detail {
  margin: 0 !important;
  font-size: 13.5px;
  line-height: 1.55;
  color: var(--cg-muted);
}
.cg-bench-foot {
  margin: 16px 0 0 !important;
  font-size: 13px;
  line-height: 1.6;
  color: var(--cg-muted);
}
.cg-bench-foot a {
  color: var(--cg-accent);
  font-weight: 600;
}
@media (max-width: 860px) {
  .cg-bench-grid {
    grid-template-columns: 1fr;
  }
}
</style>
