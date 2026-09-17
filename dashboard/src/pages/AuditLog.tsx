import { useCallback, useEffect, useState } from 'react'
import { Search, ChevronLeft, ChevronRight, Plus, Minus, ArrowUpRight, Archive, ArchiveRestore } from 'lucide-react'
import { auditApi } from '../api'
import type { AuditEntry } from '../types'
import { useEnvironment, type Environment } from '../context/EnvironmentContext'

const ACTION_STYLES: Record<AuditEntry['action'], { label: string; cls: string }> = {
  CREATE:     { label: 'Created',     cls: 'bg-brand-50 text-brand-700 ring-brand-100' },
  UPDATE:     { label: 'Updated',     cls: 'bg-blue-50   text-blue-700   ring-blue-100'   },
  DELETE:     { label: 'Deleted',     cls: 'bg-red-50    text-red-700    ring-red-100'    },
  PROMOTE:    { label: 'Promoted',    cls: 'bg-violet-50 text-violet-700 ring-violet-100' },
  ARCHIVE:    { label: 'Archived',    cls: 'bg-gray-100  text-gray-600   ring-gray-200'   },
  UNARCHIVE:  { label: 'Unarchived',  cls: 'bg-amber-50  text-amber-700  ring-amber-100'  },
}

function ActionBadge({ action }: { action: AuditEntry['action'] }) {
  const s = ACTION_STYLES[action]
  return (
    <span
      className={`inline-flex items-center px-2 py-0.5 rounded text-[10px] font-bold uppercase tracking-wide ring-1 ${s.cls}`}
    >
      {s.label}
    </span>
  )
}

function ActionIcon({ action }: { action: AuditEntry['action'] }) {
  if (action === 'CREATE')    return <Plus className="w-3.5 h-3.5 text-brand-500" />
  if (action === 'DELETE')    return <Minus className="w-3.5 h-3.5 text-red-500" />
  if (action === 'PROMOTE')   return <ArrowUpRight className="w-3.5 h-3.5 text-violet-500" />
  if (action === 'ARCHIVE')   return <Archive className="w-3.5 h-3.5 text-gray-500" />
  if (action === 'UNARCHIVE') return <ArchiveRestore className="w-3.5 h-3.5 text-amber-500" />
  return (
    <span className="w-3.5 h-3.5 flex items-center justify-center text-[10px] font-bold text-blue-500">
      ~
    </span>
  )
}

function formatDate(iso: string) {
  try {
    return new Intl.DateTimeFormat(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    }).format(new Date(iso))
  } catch {
    return iso
  }
}

// ---------------------------------------------------------------------------
// Diff viewer — shows which top-level flag fields changed
// ---------------------------------------------------------------------------

function ChangeSummary({ entry }: { entry: AuditEntry }) {
  if (entry.action === 'DELETE' || entry.action === 'CREATE') return null
  if (!entry.before_data || !entry.after_data) return null

  const before = entry.before_data
  const after  = entry.after_data
  const changed: string[] = []

  for (const k of new Set([...Object.keys(before), ...Object.keys(after)])) {
    if (JSON.stringify(before[k]) !== JSON.stringify(after[k])) changed.push(k)
  }

  if (changed.length === 0) return null

  return (
    <div className="mt-1 flex flex-wrap gap-1">
      {changed.map(k => (
        <span
          key={k}
          className="inline-flex items-center gap-1 px-1.5 py-0.5 bg-gray-100 rounded text-[9px] font-mono text-gray-500"
        >
          {k}
        </span>
      ))}
    </div>
  )
}

function MetaSummary({ entry, envMap }: { entry: AuditEntry; envMap: Map<string, string> }) {
  if (entry.action !== 'PROMOTE' || !entry.metadata) return null
  const meta = entry.metadata as Record<string, string>
  const toId = meta.to_env_id
  const fromId = meta.from_env_id
  if (!toId) return null
  const toName = envMap.get(toId) ?? toId.slice(0, 8) + '…'
  const fromName = fromId ? (envMap.get(fromId) ?? fromId.slice(0, 8) + '…') : null
  return (
    <p className="text-[10px] text-gray-400 mt-0.5">
      {fromName && <span>from <span className="font-medium text-gray-500">{fromName}</span> → </span>}
      to <span className="font-medium text-gray-500">{toName}</span>
    </p>
  )
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

const PAGE_SIZE = 50

export default function AuditLog() {
  const { activeEnv, environments } = useEnvironment()
  const [entries, setEntries]   = useState<AuditEntry[]>([])
  const [loading, setLoading]   = useState(true)
  const [error, setError]       = useState<string | null>(null)
  const [flagKey, setFlagKey]   = useState('')
  const [page, setPage]         = useState(0)
  const [hasMore, setHasMore]   = useState(false)

  // Map from env UUID → human name, used to label PROMOTE entries.
  const envMap = new Map<string, string>(
    (environments as Environment[]).map((e) => [e.id, e.name]),
  )

  const load = useCallback(
    async (offset: number, filterKey: string) => {
      if (!activeEnv) return
      setLoading(true)
      setError(null)
      try {
        const data = await auditApi.list(activeEnv.id, {
          flagKey: filterKey.trim() || undefined,
          limit: PAGE_SIZE + 1,
          offset,
        })
        setHasMore(data.length > PAGE_SIZE)
        setEntries(data.slice(0, PAGE_SIZE))
      } catch (e) {
        setError(e instanceof Error ? e.message : 'Failed to load audit log')
      } finally {
        setLoading(false)
      }
    },
    [activeEnv],
  )

  useEffect(() => {
    setPage(0)
    void load(0, flagKey)
  }, [activeEnv, load]) // eslint-disable-line react-hooks/exhaustive-deps

  function handleSearch(e: React.FormEvent) {
    e.preventDefault()
    setPage(0)
    void load(0, flagKey)
  }

  function goNext() {
    const next = page + 1
    setPage(next)
    void load(next * PAGE_SIZE, flagKey)
  }

  function goPrev() {
    const prev = Math.max(0, page - 1)
    setPage(prev)
    void load(prev * PAGE_SIZE, flagKey)
  }

  return (
    <div className="w-full space-y-4">
      {/* Toolbar */}
      <form onSubmit={handleSearch} className="flex items-center gap-3">
        <div className="relative flex-1 max-w-sm">
          <Search className="absolute left-3.5 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400 pointer-events-none" />
          <input
            type="search"
            placeholder="Filter by flag key…"
            value={flagKey}
            onChange={e => setFlagKey(e.target.value)}
            className="w-full bg-white border border-gray-100 rounded-xl pl-10 pr-4 py-2.5 text-sm text-gray-900 placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-brand-500/20 focus:border-brand-500/40 shadow-premium transition-all"
          />
        </div>
        <button
          type="submit"
          className="px-4 py-2.5 bg-brand-600 hover:bg-brand-700 text-white text-sm font-bold rounded-xl transition-all shadow-sm"
        >
          Search
        </button>

        {activeEnv && (
          <div className="ml-auto flex items-center gap-1.5 px-3 py-1.5 bg-white border border-gray-100 rounded-xl text-xs font-semibold text-gray-600 shadow-sm">
            <div className="w-2 h-2 rounded-full" style={{ backgroundColor: activeEnv.color }} />
            {activeEnv.name}
          </div>
        )}
      </form>

      {/* Log card */}
      <div className="premium-card shadow-premium-lg border-none bg-white">
        {loading ? (
          <div className="flex items-center justify-center h-40 text-gray-400 text-sm">
            Loading…
          </div>
        ) : error ? (
          <div className="flex items-center justify-center h-40 text-red-500 text-sm">{error}</div>
        ) : entries.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-40 gap-2">
            <p className="text-gray-400 text-sm">
              {flagKey ? `No events for "${flagKey}"` : 'No audit events yet.'}
            </p>
            <p className="text-gray-300 text-xs">
              Flag changes will appear here automatically.
            </p>
          </div>
        ) : (
          <div className="divide-y divide-gray-50">
            {entries.map(entry => (
              <div key={entry.id} className="flex items-start gap-4 px-6 py-4 hover:bg-gray-50/50 transition-colors">
                {/* Icon */}
                <div className="mt-0.5 w-6 h-6 rounded-full bg-gray-50 border border-gray-100 flex items-center justify-center shrink-0">
                  <ActionIcon action={entry.action} />
                </div>

                {/* Content */}
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="font-mono text-sm font-semibold text-brand-600">
                      {entry.flag_key}
                    </span>
                    <ActionBadge action={entry.action} />
                  </div>
                  <ChangeSummary entry={entry} />
                  <MetaSummary entry={entry} envMap={envMap} />
                </div>

                {/* Meta */}
                <div className="text-right shrink-0">
                  <p className="text-xs text-gray-500 font-medium">
                    {entry.actor_email ?? <span className="italic text-gray-300">SDK key</span>}
                  </p>
                  <p className="text-[10px] text-gray-300 mt-0.5">{formatDate(entry.created_at)}</p>
                </div>
              </div>
            ))}
          </div>
        )}

        {/* Pagination */}
        {!loading && !error && (entries.length > 0 || page > 0) && (
          <div className="px-6 py-3 border-t border-gray-100 flex items-center justify-between text-xs text-gray-400 bg-gray-50">
            <span>
              Page {page + 1}
              {flagKey && ` · filtered by "${flagKey}"`}
            </span>
            <div className="flex items-center gap-1">
              <button
                onClick={goPrev}
                disabled={page === 0}
                className="p-1.5 rounded-lg hover:bg-gray-200 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
              >
                <ChevronLeft className="w-3.5 h-3.5" />
              </button>
              <button
                onClick={goNext}
                disabled={!hasMore}
                className="p-1.5 rounded-lg hover:bg-gray-200 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
              >
                <ChevronRight className="w-3.5 h-3.5" />
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
