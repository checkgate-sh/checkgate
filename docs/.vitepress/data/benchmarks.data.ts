// Build-time loader: the few published benchmark figures the site shows,
// read from the committed results so a number on a page can never drift from
// the raw data. Only these fields reach the browser bundle.
//
// The results directory is the last version benchmarked with the public suite
// (bench/public/run.sh). Point it at a newer one only after that version's
// results are committed under docs/performance/results/.
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const RESULTS = '../../performance/results/v0.1.31/summary.json'

export interface Benchmarks {
  version: string
  date: string
  commit: string
  host: string
  serverLimits: string
  baselineVersion: string | null
  evaluation: { medianNs: number; p95Ns: number; p99Ns: number; rustCoreMedianNs: number; scenario: string; baselineNs: number | null }
  throughput: { rps: number; concurrency: number; cpuMsPerRequest: number; rssPeakMib: number; baselineRps: number | null }
  propagation: { clients: number; p50Ms: number; p95Ms: number; p99Ms: number; runsP99MedianMs: number; runs: number; deliveries: number; missed: number }
}

declare const data: Benchmarks
export { data }

export default {
  watch: [RESULTS],
  load(): Benchmarks {
    const s = JSON.parse(readFileSync(fileURLToPath(new URL(RESULTS, import.meta.url)), 'utf8'))
    const h = s.headline
    const rust = s.evaluation.rust_core.cases?.percentage_rollout ?? null
    const level = s.throughput.levels.find((l: any) => l.concurrency === 1000)
    const prof = s.propagation.profiles.find((p: any) => p.clients === 1000)
    const v = prof.visible_from_request_ms
    return {
      version: s.version,
      date: s.date,
      commit: s.commit.slice(0, 7),
      host: s.environment.short_host,
      serverLimits: s.environment.server_limits,
      baselineVersion: s.baseline?.version ?? null,
      evaluation: {
        medianNs: h.evaluation.value,
        p95Ns: h.evaluation.p95,
        p99Ns: h.evaluation.p99,
        rustCoreMedianNs: rust?.median_ns ?? NaN,
        scenario: h.evaluation.scenario,
        baselineNs: h.evaluation.baseline_value ?? null,
      },
      throughput: {
        rps: level.rps_median,
        concurrency: level.concurrency,
        cpuMsPerRequest: 1000 / level.requests_per_server_cpu_second,
        rssPeakMib: level.server_rss_mib_peak,
        baselineRps: h.throughput.baseline_value ?? null,
      },
      propagation: {
        clients: prof.clients,
        p50Ms: v.p50_ms,
        p95Ms: v.p95_ms,
        p99Ms: v.p99_ms,
        runsP99MedianMs: h.propagation.runs_p99_median_ms,
        runs: prof.runs ?? s.propagation.runs,
        deliveries: prof.expected_observations,
        missed: prof.missed,
      },
    }
  },
}
