// Number formatting for measured values, matching the README graphics
// (bench/public/scripts/render_svg.py fmt_value): three significant figures.

function sig3(v: number): number {
  if (!v) return 0
  const digits = 2 - Math.floor(Math.log10(Math.abs(v)))
  const f = Math.pow(10, digits)
  return Math.round(v * f) / f
}

/** Nanoseconds → "303 ns", "1.0 µs", "12.3 µs". */
export function fmtNs(ns: number): { num: string; unit: string } {
  if (ns >= 1_000_000) return { num: String(sig3(ns / 1_000_000)), unit: 'ms' }
  if (ns >= 10_000) return { num: String(sig3(ns / 1000)), unit: 'µs' }
  if (ns >= 1000) return { num: (ns / 1000).toFixed(1), unit: 'µs' }
  return { num: String(Math.round(ns)), unit: 'ns' }
}

export function fmtMs(ms: number): { num: string; unit: string } {
  return { num: ms < 10 ? ms.toFixed(1) : String(Math.round(ms)), unit: 'ms' }
}

export const fmtInt = (v: number) => Math.round(v).toLocaleString('en-US')
export const fmtSig = (v: number) => sig3(v).toLocaleString('en-US')
