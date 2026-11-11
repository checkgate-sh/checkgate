// Small shared helpers for the benchmark scripts. No dependencies.

import { readFileSync } from 'node:fs';

/** `--name value` pairs over `defaults`; unknown names are an error. */
export function parseArgs(defaults, argv = process.argv.slice(2)) {
  const out = { ...defaults };
  for (let i = 0; i < argv.length; i += 2) {
    const name = argv[i].replace(/^--/, '');
    if (!(name in defaults)) throw new Error(`unknown argument --${name}`);
    if (i + 1 >= argv.length) throw new Error(`--${name} needs a value`);
    out[name] = argv[i + 1];
  }
  return out;
}

export const list = (s) => String(s).split(',').map((v) => Number(v.trim()));

/** Nearest-rank percentile of a sorted array. */
export function pct(sorted, p) {
  if (sorted.length === 0) return null;
  return sorted[Math.round((sorted.length - 1) * p)];
}

export const round3 = (v) => (v === null ? null : Math.round(v * 1000) / 1000);

export function summarize(values) {
  const s = Float64Array.from(values).sort();
  if (s.length === 0) return { samples: 0 };
  let sum = 0;
  for (const v of s) sum += v;
  return {
    samples: s.length,
    p50_ms: round3(pct(s, 0.5)), p90_ms: round3(pct(s, 0.9)), p95_ms: round3(pct(s, 0.95)),
    p99_ms: round3(pct(s, 0.99)), p999_ms: round3(pct(s, 0.999)),
    max_ms: round3(s[s.length - 1]), min_ms: round3(s[0]), mean_ms: round3(sum / s.length),
  };
}

/** Sparse histogram in 0.1 ms buckets (key = floor(ms * 10)), mergeable across runs. */
export function histogram(values) {
  const h = {};
  for (const v of values) {
    const b = Math.floor(v * 10);
    h[b] = (h[b] || 0) + 1;
  }
  return h;
}

/**
 * (user+system CPU seconds, VmRSS bytes, VmHWM bytes) of a process from
 * /proc, or null where /proc is unavailable (macOS) or the PID is not in our
 * PID namespace. Linux reports CPU in USER_HZ = 100.
 */
export function procUsage(pid) {
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, 'utf8');
    const f = stat.slice(stat.lastIndexOf(')') + 2).split(' ');
    const cpu = (Number(f[11]) + Number(f[12])) / 100;
    const status = readFileSync(`/proc/${pid}/status`, 'utf8');
    const kb = (k) => Number((status.match(new RegExp(`^${k}:\\s+(\\d+)`, 'm')) || [])[1]) * 1024;
    return { cpu, rss: kb('VmRSS'), hwm: kb('VmHWM') };
  } catch {
    return null;
  }
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export const mib = (b) => (b == null ? null : round3(b / (1024 * 1024)));
