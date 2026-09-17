import { useEffect, useState, useCallback, useRef } from 'react'
import {
  Activity, Users, TrendingUp, Search, ChevronLeft, ChevronRight,
  Radio, Pause, Play, Trash2, ChevronDown, ChevronRight as ChevronRightSm,
} from 'lucide-react'
import { api } from '../api'
import type { Impression, ImpressionStats } from '../types'
import { useEnvironment } from '../context/EnvironmentContext'

const PAGE_SIZE = 50
const STREAM_PAGE_SIZE = 100
const MAX_STREAM_ITEMS = 500

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

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

function ValueBadge({ value }: { value: string }) {
  const isTrue = value === 'true'
  const isFalse = value === 'false'
  const color = isTrue
    ? 'bg-brand-50 text-brand-700 ring-1 ring-brand-100'
    : isFalse
    ? 'bg-gray-100 text-gray-500 ring-1 ring-gray-200'
    : 'bg-blue-50 text-blue-700 ring-1 ring-blue-100'
  return (
    <span className={`inline-flex items-center px-2 py-0.5 rounded-full text-[10px] font-bold uppercase tracking-wide ${color}`}>
      {value}
    </span>
  )
}

function formatTime(iso: string) {
  return new Date(iso).toLocaleString(undefined, {
    month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit',
  })
}

function relativeTime(iso: string) {
  const diff = Date.now() - new Date(iso).getTime()
  if (diff < 5000) return 'just now'
  if (diff < 60000) return `${Math.floor(diff / 1000)}s ago`
  if (diff < 3600000) return `${Math.floor(diff / 60000)}m ago`
  return formatTime(iso)
}

// ---------------------------------------------------------------------------
// Analytics tab
// ---------------------------------------------------------------------------

function StatsTable({ stats }: { stats: ImpressionStats[] }) {
  if (stats.length === 0) {
    return (
      <div className="flex items-center justify-center h-32 text-gray-400 text-sm">
        No evaluations recorded yet.
      </div>
    )
  }
  const maxTotal = Math.max(...stats.map(s => s.total), 1)
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-gray-50 bg-gray-50/50">
            <th className="text-left px-6 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Flag</th>
            <th className="text-left px-6 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Evaluations</th>
            <th className="text-left px-6 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden sm:table-cell">True / False</th>
            <th className="text-left px-6 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden md:table-cell">Unique users</th>
            <th className="text-left px-6 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden lg:table-cell">Last seen</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-gray-50/50">
          {stats.map(s => {
            const trueRatio = s.total > 0 ? (s.true_count / s.total) * 100 : 0
            const barWidth = (s.total / maxTotal) * 100
            return (
              <tr key={s.flag_key} className="hover:bg-brand-50/20 transition-all">
                <td className="px-6 py-4">
                  <span className="font-mono text-brand-600 font-semibold text-sm">{s.flag_key}</span>
                </td>
                <td className="px-6 py-4">
                  <div className="flex items-center gap-3">
                    <span className="font-bold text-gray-900 tabular-nums w-12">{s.total.toLocaleString()}</span>
                    <div className="flex-1 max-w-[120px] h-1.5 bg-gray-100 rounded-full overflow-hidden">
                      <div className="h-full bg-brand-500 rounded-full" style={{ width: `${barWidth}%` }} />
                    </div>
                  </div>
                </td>
                <td className="px-6 py-4 hidden sm:table-cell">
                  <div className="flex items-center gap-1.5">
                    <div className="flex-1 max-w-[80px] h-2 bg-gray-100 rounded-full overflow-hidden flex">
                      <div className="h-full bg-brand-400 rounded-l-full" style={{ width: `${trueRatio}%` }} />
                      <div className="h-full bg-gray-300 rounded-r-full" style={{ width: `${100 - trueRatio}%` }} />
                    </div>
                    <span className="text-xs text-gray-500 tabular-nums">
                      {s.true_count.toLocaleString()} / {s.false_count.toLocaleString()}
                    </span>
                  </div>
                </td>
                <td className="px-6 py-4 hidden md:table-cell text-gray-600 tabular-nums">
                  {s.unique_users.toLocaleString()}
                </td>
                <td className="px-6 py-4 hidden lg:table-cell text-gray-400 text-xs">
                  {s.last_seen ? formatTime(s.last_seen) : '—'}
                </td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}

function RecentTable({
  items, total, offset, onPage,
}: {
  items: Impression[]; total: number; offset: number; onPage: (n: number) => void
}) {
  const page = Math.floor(offset / PAGE_SIZE) + 1
  const totalPages = Math.ceil(total / PAGE_SIZE)
  if (items.length === 0 && total === 0) {
    return <div className="flex items-center justify-center h-32 text-gray-400 text-sm">No evaluations recorded yet.</div>
  }
  return (
    <>
      <div className="overflow-x-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-50 bg-gray-50/50">
              <th className="text-left px-6 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Time</th>
              <th className="text-left px-6 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Flag</th>
              <th className="text-left px-6 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Value</th>
              <th className="text-left px-6 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden sm:table-cell">User</th>
              <th className="text-left px-6 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden lg:table-cell">Context</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-50/50">
            {items.map(imp => (
              <tr key={imp.id} className="hover:bg-brand-50/20 transition-all">
                <td className="px-6 py-3.5 text-gray-400 text-xs tabular-nums whitespace-nowrap">{formatTime(imp.evaluated_at)}</td>
                <td className="px-6 py-3.5">
                  <span className="font-mono text-brand-600 font-semibold text-xs">{imp.flag_key}</span>
                </td>
                <td className="px-6 py-3.5"><ValueBadge value={imp.value} /></td>
                <td className="px-6 py-3.5 hidden sm:table-cell text-gray-500 text-xs font-mono">
                  {imp.user_id ?? <span className="text-gray-300">anonymous</span>}
                </td>
                <td className="px-6 py-3.5 hidden lg:table-cell">
                  {imp.context
                    ? <code className="text-[10px] text-gray-400 bg-gray-50 px-1.5 py-0.5 rounded max-w-[200px] truncate block">{JSON.stringify(imp.context)}</code>
                    : <span className="text-gray-300">—</span>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {totalPages > 1 && (
        <div className="px-6 py-3 border-t border-gray-100 bg-gray-50 flex items-center justify-between text-xs text-gray-400">
          <span>{offset + 1}–{Math.min(offset + PAGE_SIZE, total)} of {total.toLocaleString()}</span>
          <div className="flex items-center gap-1">
            <button onClick={() => onPage(offset - PAGE_SIZE)} disabled={page <= 1} className="p-1.5 rounded-lg hover:bg-gray-200 disabled:opacity-30 disabled:cursor-not-allowed transition-colors">
              <ChevronLeft className="w-3.5 h-3.5" />
            </button>
            <span className="px-2 font-semibold text-gray-600">{page} / {totalPages}</span>
            <button onClick={() => onPage(offset + PAGE_SIZE)} disabled={page >= totalPages} className="p-1.5 rounded-lg hover:bg-gray-200 disabled:opacity-30 disabled:cursor-not-allowed transition-colors">
              <ChevronRight className="w-3.5 h-3.5" />
            </button>
          </div>
        </div>
      )}
    </>
  )
}

// ---------------------------------------------------------------------------
// Evaluation Stream tab
// ---------------------------------------------------------------------------

const POLL_INTERVAL_MS = 3000

function EvaluationStream() {
  const { activeEnv } = useEnvironment()
  const [items, setItems] = useState<Impression[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [live, setLive] = useState(true)
  const [newIds, setNewIds] = useState<Set<number>>(new Set())
  const [expanded, setExpanded] = useState<Set<number>>(new Set())

  // Filters
  const [flagFilter, setFlagFilter] = useState('')
  const [userFilter, setUserFilter] = useState('')
  const [valueFilter, setValueFilter] = useState('')

  // Ref so polling closure always sees the latest sinceId without re-creating the interval.
  const sinceIdRef = useRef<number | null>(null)

  const loadInitial = useCallback(async () => {
    if (!activeEnv) return
    setLoading(true)
    setError(null)
    sinceIdRef.current = null
    try {
      const res = await api.listImpressions(activeEnv.id, {
        flagKey: flagFilter.trim() || undefined,
        userId: userFilter.trim() || undefined,
        value: valueFilter || undefined,
        limit: STREAM_PAGE_SIZE,
      })
      setItems(res.items)
      if (res.items.length > 0) {
        sinceIdRef.current = Math.max(...res.items.map(i => i.id))
      }
      setNewIds(new Set())
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load evaluations')
    } finally {
      setLoading(false)
    }
  }, [activeEnv, flagFilter, userFilter, valueFilter])

  // Reset and reload whenever env or filters change.
  useEffect(() => {
    setItems([])
    setExpanded(new Set())
    void loadInitial()
  }, [loadInitial])

  // Live polling — fires every 3s, uses sinceIdRef to only fetch new rows.
  useEffect(() => {
    if (!live) return
    const poll = async () => {
      if (!activeEnv) return
      try {
        const res = await api.listImpressions(activeEnv.id, {
          flagKey: flagFilter.trim() || undefined,
          userId: userFilter.trim() || undefined,
          value: valueFilter || undefined,
          sinceId: sinceIdRef.current ?? undefined,
          limit: STREAM_PAGE_SIZE,
        })
        if (res.items.length === 0) return
        const incoming = res.items
        const ids = new Set(incoming.map(i => i.id))
        const maxId = Math.max(...incoming.map(i => i.id))
        if (maxId > (sinceIdRef.current ?? 0)) sinceIdRef.current = maxId
        setItems(prev => [...incoming, ...prev.filter(i => !ids.has(i.id))].slice(0, MAX_STREAM_ITEMS))
        setNewIds(ids)
        setTimeout(() => setNewIds(new Set()), 1500)
      } catch {
        // ignore transient poll errors
      }
    }
    const timer = setInterval(() => void poll(), POLL_INTERVAL_MS)
    return () => clearInterval(timer)
  }, [live, activeEnv, flagFilter, userFilter, valueFilter])

  function toggleExpanded(id: number) {
    setExpanded(prev => {
      const next = new Set(prev)
      next.has(id) ? next.delete(id) : next.add(id)
      return next
    })
  }

  function handleClear() {
    setItems([])
    sinceIdRef.current = null
    setNewIds(new Set())
    setExpanded(new Set())
  }

  const inputClass = 'bg-gray-50 border border-gray-100 rounded-xl px-3 py-2 text-sm text-gray-900 placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-brand-500/20 focus:border-brand-400/30 transition-all'

  return (
    <div className="premium-card bg-white shadow-premium-lg border-none">
      {/* Toolbar */}
      <div className="px-5 py-3.5 border-b border-gray-50 flex flex-wrap items-center gap-3">
        {/* Live indicator + toggle */}
        <button
          onClick={() => setLive(l => !l)}
          className={`flex items-center gap-2 px-3 py-1.5 rounded-lg text-xs font-semibold transition-all ${
            live
              ? 'bg-brand-50 text-brand-700 hover:bg-brand-100'
              : 'bg-gray-100 text-gray-500 hover:bg-gray-200'
          }`}
        >
          {live ? (
            <>
              <span className="relative flex w-2 h-2">
                <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-brand-400 opacity-75" />
                <span className="relative inline-flex rounded-full h-2 w-2 bg-brand-500" />
              </span>
              <Pause className="w-3 h-3" /> Live
            </>
          ) : (
            <>
              <Radio className="w-3 h-3" />
              <Play className="w-3 h-3" /> Paused
            </>
          )}
        </button>

        {/* Filters */}
        <div className="relative">
          <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3 h-3 text-gray-400 pointer-events-none" />
          <input
            type="search"
            placeholder="Flag key…"
            value={flagFilter}
            onChange={e => setFlagFilter(e.target.value)}
            className={`${inputClass} pl-7 w-36`}
          />
        </div>

        <div className="relative">
          <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3 h-3 text-gray-400 pointer-events-none" />
          <input
            type="search"
            placeholder="User ID…"
            value={userFilter}
            onChange={e => setUserFilter(e.target.value)}
            className={`${inputClass} pl-7 w-36`}
          />
        </div>

        <div className="relative">
          <select
            value={valueFilter}
            onChange={e => setValueFilter(e.target.value)}
            className={`${inputClass} pr-7 appearance-none w-28`}
          >
            <option value="">Any value</option>
            <option value="true">true</option>
            <option value="false">false</option>
          </select>
          <ChevronDown className="absolute right-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-gray-400 pointer-events-none" />
        </div>

        <div className="flex-1" />

        {/* Item count */}
        {items.length > 0 && (
          <span className="text-xs text-gray-400 tabular-nums">
            {items.length.toLocaleString()}{items.length >= MAX_STREAM_ITEMS ? ` (capped)` : ''} events
          </span>
        )}

        <button
          onClick={handleClear}
          disabled={items.length === 0}
          className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-xs text-gray-400 hover:text-rose-500 hover:bg-rose-50 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
          title="Clear stream"
        >
          <Trash2 className="w-3.5 h-3.5" /> Clear
        </button>
      </div>

      {/* Stream body */}
      {error && (
        <div className="px-5 py-3 text-sm text-rose-500 bg-rose-50 border-b border-rose-100">{error}</div>
      )}

      {loading ? (
        <div className="flex items-center justify-center h-32 text-gray-400 text-sm">Loading…</div>
      ) : items.length === 0 ? (
        <div className="flex flex-col items-center justify-center h-40 text-gray-400 text-sm gap-2">
          <Radio className="w-6 h-6 text-gray-200" />
          <p>No evaluations yet — waiting for SDK traffic</p>
          {!live && <p className="text-xs">Resume live mode to see new events</p>}
        </div>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-50 bg-gray-50/50">
                <th className="w-6 px-3 py-3" />
                <th className="text-left px-4 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest whitespace-nowrap">Time</th>
                <th className="text-left px-4 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Flag</th>
                <th className="text-left px-4 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Value</th>
                <th className="text-left px-4 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden sm:table-cell">User</th>
                <th className="text-left px-4 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden md:table-cell">Context</th>
              </tr>
            </thead>
            <tbody>
              {items.map(imp => {
                const isNew = newIds.has(imp.id)
                const isOpen = expanded.has(imp.id)
                const hasCtx = imp.context != null && Object.keys(imp.context).length > 0
                return (
                  <>
                    <tr
                      key={imp.id}
                      className={`border-b border-gray-50 transition-colors duration-1000 ${
                        isNew ? 'bg-amber-50' : 'hover:bg-gray-50/50'
                      }`}
                    >
                      {/* Expand toggle */}
                      <td className="pl-3 pr-1 py-3">
                        {hasCtx && (
                          <button
                            onClick={() => toggleExpanded(imp.id)}
                            className="p-0.5 rounded text-gray-300 hover:text-gray-600 transition-colors"
                          >
                            {isOpen
                              ? <ChevronDown className="w-3.5 h-3.5" />
                              : <ChevronRightSm className="w-3.5 h-3.5" />}
                          </button>
                        )}
                      </td>
                      <td className="px-4 py-3 text-gray-400 text-xs tabular-nums whitespace-nowrap" title={formatTime(imp.evaluated_at)}>
                        {relativeTime(imp.evaluated_at)}
                      </td>
                      <td className="px-4 py-3">
                        <span className="font-mono text-brand-600 font-semibold text-xs">{imp.flag_key}</span>
                      </td>
                      <td className="px-4 py-3">
                        <ValueBadge value={imp.value} />
                      </td>
                      <td className="px-4 py-3 hidden sm:table-cell text-xs font-mono text-gray-500 max-w-[140px] truncate">
                        {imp.user_id ?? <span className="text-gray-300">anonymous</span>}
                      </td>
                      <td className="px-4 py-3 hidden md:table-cell">
                        {hasCtx && !isOpen && (
                          <button
                            onClick={() => toggleExpanded(imp.id)}
                            className="text-[10px] text-gray-400 bg-gray-50 hover:bg-gray-100 px-1.5 py-0.5 rounded max-w-[180px] truncate block transition-colors text-left"
                          >
                            {JSON.stringify(imp.context)}
                          </button>
                        )}
                        {!hasCtx && <span className="text-gray-300 text-xs">—</span>}
                      </td>
                    </tr>
                    {isOpen && hasCtx && (
                      <tr key={`${imp.id}-ctx`} className="bg-gray-50/50 border-b border-gray-50">
                        <td colSpan={6} className="px-10 py-3">
                          <pre className="text-[11px] text-gray-600 bg-white border border-gray-100 rounded-lg p-3 overflow-x-auto leading-relaxed">
                            {JSON.stringify(imp.context, null, 2)}
                          </pre>
                        </td>
                      </tr>
                    )}
                  </>
                )
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

type Tab = 'analytics' | 'stream'

export default function Impressions() {
  const { activeEnv } = useEnvironment()
  const [tab, setTab] = useState<Tab>('analytics')
  const [stats, setStats] = useState<ImpressionStats[]>([])
  const [items, setItems] = useState<Impression[]>([])
  const [total, setTotal] = useState(0)
  const [offset, setOffset] = useState(0)
  const [flagFilter, setFlagFilter] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const loadStats = useCallback(async () => {
    if (!activeEnv) return
    setStats(await api.impressionStats(activeEnv.id))
  }, [activeEnv])

  const loadStream = useCallback(async () => {
    if (!activeEnv) return
    const res = await api.listImpressions(activeEnv.id, {
      flagKey: flagFilter.trim() || undefined,
      limit: PAGE_SIZE,
      offset,
    })
    setItems(res.items)
    setTotal(res.total)
  }, [activeEnv, flagFilter, offset])

  const load = useCallback(async () => {
    if (!activeEnv) return
    setLoading(true)
    setError(null)
    try {
      await Promise.all([loadStats(), loadStream()])
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load impressions')
    } finally {
      setLoading(false)
    }
  }, [activeEnv, loadStats, loadStream])

  useEffect(() => { setOffset(0) }, [flagFilter, activeEnv])
  useEffect(() => { void load() }, [load])

  const totalEvals = stats.reduce((sum, s) => sum + s.total, 0)
  const totalUsers = stats.reduce((sum, s) => sum + s.unique_users, 0)
  const topFlag = stats[0]?.flag_key ?? '—'

  return (
    <div className="w-full space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-display font-bold text-gray-900">Impressions</h1>
          <p className="text-sm text-gray-400 mt-0.5">
            Flag evaluation events reported by SDK clients
            {activeEnv && (
              <span className="ml-2 inline-flex items-center gap-1">
                <span className="w-2 h-2 rounded-full inline-block" style={{ backgroundColor: activeEnv.color }} />
                {activeEnv.name}
              </span>
            )}
          </p>
        </div>

        {/* Tab switcher */}
        <div className="flex gap-1 p-1 bg-gray-100 rounded-xl">
          {(['analytics', 'stream'] as Tab[]).map(t => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={`flex items-center gap-1.5 px-4 py-2 rounded-lg text-sm font-semibold transition-all capitalize ${
                tab === t ? 'bg-white text-gray-900 shadow-sm' : 'text-gray-500 hover:text-gray-700'
              }`}
            >
              {t === 'analytics' ? <TrendingUp className="w-3.5 h-3.5" /> : <Radio className="w-3.5 h-3.5" />}
              {t === 'analytics' ? 'Analytics' : 'Stream'}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="px-4 py-3 bg-rose-50 border border-rose-100 rounded-xl text-rose-600 text-sm">{error}</div>
      )}

      {tab === 'analytics' && (
        <>
          {/* Stats cards */}
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
            <StatCard label="Total evaluations" value={totalEvals.toLocaleString()} icon={Activity} />
            <StatCard label="Unique users" value={totalUsers.toLocaleString()} icon={Users} sub="across all flags" />
            <StatCard
              label="Most evaluated"
              value={topFlag}
              icon={TrendingUp}
              sub={topFlag !== '—' ? `${stats[0]?.total.toLocaleString()} evals` : undefined}
            />
          </div>

          {/* Per-flag stats */}
          <div className="premium-card bg-white shadow-premium-lg border-none">
            <div className="px-6 py-4 border-b border-gray-50">
              <h2 className="font-bold text-gray-900">By flag</h2>
            </div>
            {loading
              ? <div className="flex items-center justify-center h-32 text-gray-400 text-sm">Loading…</div>
              : <StatsTable stats={stats} />}
          </div>

          {/* Recent evaluations */}
          <div className="premium-card bg-white shadow-premium-lg border-none">
            <div className="px-6 py-4 border-b border-gray-50 flex items-center gap-3">
              <h2 className="font-bold text-gray-900 flex-1">Recent evaluations</h2>
              <div className="relative">
                <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-gray-400 pointer-events-none" />
                <input
                  type="search"
                  placeholder="Filter by flag…"
                  value={flagFilter}
                  onChange={e => setFlagFilter(e.target.value)}
                  className="bg-gray-50 border border-gray-100 rounded-xl pl-9 pr-4 py-2 text-sm text-gray-900 placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-brand-500/20 w-52"
                />
              </div>
            </div>
            {loading
              ? <div className="flex items-center justify-center h-32 text-gray-400 text-sm">Loading…</div>
              : <RecentTable items={items} total={total} offset={offset} onPage={setOffset} />}
          </div>
        </>
      )}

      {tab === 'stream' && <EvaluationStream />}
    </div>
  )
}
