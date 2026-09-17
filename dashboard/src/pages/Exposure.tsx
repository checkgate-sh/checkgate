import { useEffect, useState, useCallback, useMemo } from 'react'
import { PieChart, Users, Activity, Layers } from 'lucide-react'
import { api } from '../api'
import type { ExposureResponse, ImpressionStats } from '../types'
import { useEnvironment } from '../context/EnvironmentContext'

// A fixed, reasonably color-blind-safe categorical palette. Variants are
// assigned colors by their exposure rank (index into the ordered variants
// array) so the same value keeps a stable color across both charts on the page.
// Indigo is reserved for the brand and enabled state, so it is excluded from
// the categorical palette to keep variants distinct from controls.
const PALETTE = [
  '#0ea5e9', // sky
  '#8b5cf6', // violet
  '#f59e0b', // amber
  '#ec4899', // pink
  '#06b6d4', // cyan
  '#d946ef', // fuchsia
  '#ef4444', // red
  '#64748b', // slate
]

function StatCard({
  label, value, icon: Icon, sub,
}: {
  label: string; value: string | number; icon: React.ElementType; sub?: string
}) {
  return (
    <div className="premium-card bg-white p-5 flex items-start gap-4">
      <div className="p-2.5 bg-brand-50 rounded-xl">
        <Icon className="w-5 h-5 text-brand-600" />
      </div>
      <div className="min-w-0">
        <p className="text-[11px] font-bold uppercase tracking-widest text-gray-400 mb-0.5">{label}</p>
        <p className="text-2xl font-display font-bold text-gray-900 leading-none">{value}</p>
        {sub && <p className="text-xs text-gray-400 mt-1">{sub}</p>}
      </div>
    </div>
  )
}

// Stacked-column timeline: one column per day, segments sized by each variant's
// share of that day's evaluations.
function Timeline({
  data, colorFor, order,
}: {
  data: ExposureResponse['timeline']
  colorFor: (v: string) => string
  order: string[]
}) {
  const byDay = useMemo(() => {
    const map = new Map<string, Map<string, number>>()
    for (const p of data) {
      if (!map.has(p.day)) map.set(p.day, new Map())
      map.get(p.day)!.set(p.value, (map.get(p.day)!.get(p.value) ?? 0) + p.count)
    }
    return map
  }, [data])

  const days = useMemo(() => [...byDay.keys()].sort(), [byDay])
  if (days.length === 0) {
    return <div className="flex items-center justify-center h-40 text-gray-400 text-sm">No timeline data in this window.</div>
  }

  const dayTotals = days.map(d => [...byDay.get(d)!.values()].reduce((a, b) => a + b, 0))
  const maxTotal = Math.max(...dayTotals, 1)

  const W = 720, H = 200, padB = 24, padL = 4
  const chartH = H - padB
  const gap = 6
  const colW = (W - padL * 2 - gap * (days.length - 1)) / days.length

  return (
    <div className="overflow-x-auto">
      <svg viewBox={`0 0 ${W} ${H}`} className="w-full min-w-[520px]" preserveAspectRatio="xMidYMid meet">
        {days.map((day, i) => {
          const counts = byDay.get(day)!
          const total = dayTotals[i]
          const x = padL + i * (colW + gap)
          const colHeight = (total / maxTotal) * chartH
          let yCursor = chartH - colHeight
          const segments = order
            .filter(v => counts.has(v))
            .map(v => {
              const c = counts.get(v)!
              const h = total > 0 ? (c / total) * colHeight : 0
              const seg = { v, y: yCursor, h }
              yCursor += h
              return seg
            })
          const showLabel = days.length <= 16 || i % 2 === 0
          return (
            <g key={day}>
              {segments.map((s, si) => (
                <rect
                  key={s.v}
                  x={x}
                  y={s.y}
                  width={colW}
                  height={Math.max(s.h, s.h > 0 ? 1 : 0)}
                  fill={colorFor(s.v)}
                  rx={si === 0 ? 2 : 0}
                >
                  <title>{`${day} · ${s.v}: ${counts.get(s.v)!.toLocaleString()}`}</title>
                </rect>
              ))}
              {showLabel && (
                <text
                  x={x + colW / 2}
                  y={H - 8}
                  textAnchor="middle"
                  className="fill-gray-400"
                  style={{ fontSize: 9 }}
                >
                  {day.slice(5)}
                </text>
              )}
            </g>
          )
        })}
      </svg>
    </div>
  )
}

export default function Exposure() {
  const { activeEnv } = useEnvironment()
  const [flagKeys, setFlagKeys] = useState<string[]>([])
  const [selected, setSelected] = useState<string>('')
  const [data, setData] = useState<ExposureResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  // Load the set of flags that actually have impression data to pick from.
  useEffect(() => {
    if (!activeEnv) return
    let cancelled = false
    api.impressionStats(activeEnv.id)
      .then((stats: ImpressionStats[]) => {
        if (cancelled) return
        const keys = stats.map(s => s.flag_key)
        setFlagKeys(keys)
        setSelected(prev => (prev && keys.includes(prev) ? prev : keys[0] ?? ''))
      })
      .catch(() => { if (!cancelled) setFlagKeys([]) })
    return () => { cancelled = true }
  }, [activeEnv])

  const load = useCallback(async () => {
    if (!activeEnv || !selected) { setData(null); setLoading(false); return }
    try {
      setLoading(true)
      setError(null)
      setData(await api.exposure(activeEnv.id, selected))
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load exposure')
    } finally {
      setLoading(false)
    }
  }, [activeEnv, selected])

  useEffect(() => { void load() }, [load])

  const order = useMemo(() => data?.variants.map(v => v.value) ?? [], [data])
  const colorFor = useCallback(
    (v: string) => {
      const idx = order.indexOf(v)
      return PALETTE[(idx < 0 ? 0 : idx) % PALETTE.length]
    },
    [order],
  )

  return (
    <div className="w-full space-y-6">
      {/* Header */}
      <div className="flex flex-wrap items-center justify-between gap-4">
        <div>
          <h1 className="text-xl font-display font-bold text-gray-900">Exposure</h1>
          <p className="text-sm text-gray-400 mt-0.5">Which users are being exposed to which variant.</p>
        </div>
        {flagKeys.length > 0 && (
          <select
            value={selected}
            onChange={e => setSelected(e.target.value)}
            className="px-4 py-2 rounded-xl border border-gray-200 bg-white text-sm font-mono font-semibold text-gray-700 focus:outline-none focus:ring-2 focus:ring-brand-200"
          >
            {flagKeys.map(k => <option key={k} value={k}>{k}</option>)}
          </select>
        )}
      </div>

      {flagKeys.length === 0 ? (
        <div className="premium-card bg-white flex flex-col items-center justify-center h-48 gap-2">
          <PieChart className="w-8 h-8 text-gray-300" />
          <p className="text-gray-400 text-sm">No exposure data yet — flags need reported evaluations first.</p>
        </div>
      ) : loading ? (
        <div className="flex items-center justify-center h-48 text-gray-400 text-sm">Loading…</div>
      ) : error ? (
        <div className="flex items-center justify-center h-48 text-red-500 text-sm">{error}</div>
      ) : data ? (
        <>
          {/* Stats */}
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
            <StatCard label="Total evaluations" value={data.total_impressions.toLocaleString()} icon={Activity} />
            <StatCard label="Unique users" value={data.total_users.toLocaleString()} icon={Users} />
            <StatCard label="Distinct variants" value={data.variants.length} icon={Layers} />
          </div>

          {/* Variant breakdown */}
          <div className="premium-card bg-white">
            <div className="px-6 py-4 border-b border-gray-50">
              <h2 className="text-gray-900 font-display font-bold text-base">Variant distribution</h2>
            </div>
            <div className="p-6 space-y-4">
              {data.variants.map(v => {
                const pct = data.total_impressions > 0 ? (v.impressions / data.total_impressions) * 100 : 0
                return (
                  <div key={v.value}>
                    <div className="flex items-center justify-between mb-1.5 text-sm">
                      <div className="flex items-center gap-2 min-w-0">
                        <span className="w-3 h-3 rounded-sm shrink-0" style={{ backgroundColor: colorFor(v.value) }} />
                        <span className="font-mono font-semibold text-gray-800 truncate">{v.value}</span>
                      </div>
                      <div className="flex items-center gap-4 shrink-0 text-xs text-gray-500 tabular-nums">
                        <span className="font-bold text-gray-900">{pct.toFixed(1)}%</span>
                        <span>{v.impressions.toLocaleString()} evals</span>
                        <span className="hidden sm:inline">{v.unique_users.toLocaleString()} users</span>
                      </div>
                    </div>
                    <div className="h-2.5 bg-gray-100 rounded-full overflow-hidden">
                      <div className="h-full rounded-full" style={{ width: `${pct}%`, backgroundColor: colorFor(v.value) }} />
                    </div>
                  </div>
                )
              })}
            </div>
          </div>

          {/* Timeline */}
          <div className="premium-card bg-white">
            <div className="px-6 py-4 border-b border-gray-50 flex items-center justify-between">
              <h2 className="text-gray-900 font-display font-bold text-base">Exposure over time</h2>
              <span className="text-xs text-gray-400">Last 14 days</span>
            </div>
            <div className="p-6">
              <Timeline data={data.timeline} colorFor={colorFor} order={order} />
              {/* Legend */}
              <div className="flex flex-wrap gap-x-4 gap-y-1.5 mt-4">
                {data.variants.map(v => (
                  <div key={v.value} className="flex items-center gap-1.5 text-xs text-gray-500">
                    <span className="w-2.5 h-2.5 rounded-sm" style={{ backgroundColor: colorFor(v.value) }} />
                    <span className="font-mono">{v.value}</span>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </>
      ) : null}
    </div>
  )
}
