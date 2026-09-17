import { useEffect, useState, useCallback, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { Plus, Search, RotateCcw, Pencil, Trash2, ArrowUpRight, Copy, Check, Archive, ArchiveRestore } from 'lucide-react'
import { api } from '../api'
import type { Flag, FlagType } from '../types'
import { useEnvironment } from '../context/EnvironmentContext'
import Drawer from '../components/Drawer'
import FlagForm from '../components/FlagForm'

const TYPE_BADGE: Partial<Record<FlagType, { label: string; cls: string }>> = {
  string: { label: 'STR', cls: 'bg-blue-50 text-blue-700 ring-blue-100' },
  integer: { label: 'INT', cls: 'bg-violet-50 text-violet-700 ring-violet-100' },
  json: { label: 'JSON', cls: 'bg-amber-50 text-amber-700 ring-amber-100' },
}

function TypeBadge({ flagType }: { flagType?: FlagType }) {
  if (!flagType || flagType === 'boolean') return null
  const badge = TYPE_BADGE[flagType]
  if (!badge) return null
  return (
    <span className={`inline-flex items-center px-1.5 py-0.5 rounded text-[9px] font-bold uppercase tracking-wide ring-1 ml-2 ${badge.cls}`}>
      {badge.label}
    </span>
  )
}

function Toggle({ enabled, onToggle }: { enabled: boolean; onToggle: () => void }) {
  return (
    <button
      onClick={onToggle}
      className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-brand-500/20 ${
        enabled ? 'bg-brand-600' : 'bg-gray-200'
      }`}
      aria-label={enabled ? 'Disable flag' : 'Enable flag'}
    >
      <span
        className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition-transform ${
          enabled ? 'translate-x-4' : 'translate-x-0.5'
        }`}
      />
    </button>
  )
}

// ---------------------------------------------------------------------------
// Copy-key button
// ---------------------------------------------------------------------------

function CopyKeyButton({ flagKey }: { flagKey: string }) {
  const [copied, setCopied] = useState(false)

  function handleCopy(e: React.MouseEvent) {
    e.preventDefault()
    e.stopPropagation()
    navigator.clipboard.writeText(flagKey).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    })
  }

  return (
    <button
      onClick={handleCopy}
      className="p-1.5 rounded-md text-gray-300 hover:text-gray-600 hover:bg-gray-100 transition-colors"
      aria-label="Copy key"
      title="Copy flag key"
    >
      {copied ? <Check className="w-3 h-3 text-brand-500" /> : <Copy className="w-3 h-3" />}
    </button>
  )
}

// ---------------------------------------------------------------------------
// Promote modal
// ---------------------------------------------------------------------------

function PromoteModal({ flag, onClose }: { flag: Flag; onClose: () => void }) {
  const { environments, activeEnv } = useEnvironment()
  const [targetId, setTargetId] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [done, setDone] = useState(false)

  const targets = environments.filter(e => e.id !== activeEnv?.id)

  async function handlePromote() {
    if (!activeEnv || !targetId) return
    setLoading(true)
    setError('')
    try {
      await api.promoteFlag(activeEnv.id, flag.key, targetId)
      setDone(true)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Promote failed')
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm">
      <div className="bg-white rounded-2xl shadow-2xl border border-gray-100 p-6 w-full max-w-sm mx-4">
        {done ? (
          <div className="text-center py-4">
            <p className="text-brand-600 font-bold text-lg mb-1">Promoted!</p>
            <p className="text-gray-500 text-sm mb-4">
              <code className="font-mono text-brand-600">{flag.key}</code> was copied to{' '}
              {environments.find(e => e.id === targetId)?.name}.
            </p>
            <button
              onClick={onClose}
              className="px-6 py-2 bg-brand-600 text-white font-bold rounded-xl text-sm hover:bg-brand-700 transition-colors"
            >
              Done
            </button>
          </div>
        ) : (
          <>
            <h3 className="font-bold text-gray-900 mb-1">Promote flag</h3>
            <p className="text-gray-500 text-sm mb-4">
              Copy <code className="font-mono text-brand-600">{flag.key}</code> configuration to another environment.
            </p>

            {error && (
              <p className="text-rose-500 text-xs mb-3">{error}</p>
            )}

            <div className="mb-4">
              <label className="block text-xs font-medium text-gray-600 mb-1.5">Target environment</label>
              <select
                value={targetId}
                onChange={e => setTargetId(e.target.value)}
                className="w-full bg-white border border-gray-200 rounded-lg px-3 py-2 text-sm text-gray-900 focus:outline-none focus:ring-2 focus:ring-brand-500/20"
              >
                <option value="">Select environment…</option>
                {targets.map(e => (
                  <option key={e.id} value={e.id}>{e.name}</option>
                ))}
              </select>
            </div>

            <div className="flex gap-2">
              <button
                onClick={() => void handlePromote()}
                disabled={!targetId || loading}
                className="flex-1 py-2 px-4 bg-brand-600 hover:bg-brand-700 disabled:opacity-40 disabled:cursor-not-allowed text-white text-sm font-bold rounded-xl transition-all"
              >
                {loading ? 'Promoting…' : 'Promote'}
              </button>
              <button
                onClick={onClose}
                className="px-3 py-2 bg-gray-100 hover:bg-gray-200 text-gray-600 text-sm rounded-xl transition-colors"
              >
                Cancel
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

type StatusFilter = 'all' | 'enabled' | 'disabled'

export default function FlagList() {
  const { activeEnv } = useEnvironment()
  const [searchParams, setSearchParams] = useSearchParams()
  const [flags, setFlags] = useState<Flag[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all')
  const [showArchived, setShowArchived] = useState(false)
  const [promotingFlag, setPromotingFlag] = useState<Flag | null>(null)
  const searchRef = useRef<HTMLInputElement>(null)

  const load = useCallback(async () => {
    if (!activeEnv) return
    try {
      setError(null)
      setFlags(await api.listFlags(activeEnv.id, { includeArchived: showArchived }))
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load flags')
    } finally {
      setLoading(false)
    }
  }, [activeEnv, showArchived])

  useEffect(() => {
    setLoading(true)
    void load()
  }, [load])

  // '/' or Ctrl+K focuses the search box.
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      const tag = (e.target as HTMLElement).tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return
      if (e.key === '/' || (e.key === 'k' && (e.metaKey || e.ctrlKey))) {
        e.preventDefault()
        searchRef.current?.focus()
      }
    }
    document.addEventListener('keydown', handler)
    return () => document.removeEventListener('keydown', handler)
  }, [])

  async function toggleEnabled(flag: Flag) {
    if (!activeEnv) return
    try {
      const result = await api.patchFlag(activeEnv.id, flag.key, { is_enabled: !flag.is_enabled })
      if (result.applied) {
        setFlags(prev => prev.map(f => (f.key === flag.key ? result.flag : f)))
      } else {
        alert(`${activeEnv.name} requires approval — the change to "${flag.key}" was queued for review instead of applied.`)
      }
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Update failed')
    }
  }

  async function handleDelete(key: string) {
    if (!activeEnv) return
    if (!confirm(`Delete flag "${key}" from ${activeEnv.name}? This cannot be undone.`)) return
    try {
      await api.deleteFlag(activeEnv.id, key)
      setFlags(prev => prev.filter(f => f.key !== key))
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Delete failed')
    }
  }

  async function toggleArchived(flag: Flag) {
    if (!activeEnv) return
    try {
      const updated = flag.archived_at
        ? await api.unarchiveFlag(activeEnv.id, flag.key)
        : await api.archiveFlag(activeEnv.id, flag.key)
      // Archiving hides the flag from the default (non-archived) view.
      setFlags(prev =>
        !showArchived && !flag.archived_at
          ? prev.filter(f => f.key !== flag.key)
          : prev.map(f => (f.key === flag.key ? updated : f)),
      )
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Update failed')
    }
  }

  // The panel's open/target state lives in the URL, so it survives a refresh,
  // is linkable, and closes on browser Back.
  const editingKey = searchParams.get('edit')
  const creating = searchParams.get('new') === '1'
  const panelOpen = creating || editingKey !== null

  const openNew = useCallback(() => setSearchParams({ new: '1' }), [setSearchParams])
  const openEdit = useCallback(
    (flagKey: string) => setSearchParams({ edit: flagKey }),
    [setSearchParams],
  )
  const closePanel = useCallback(() => setSearchParams({}), [setSearchParams])

  /** Merge a created/updated flag into the list without a full refetch. */
  const upsertFlag = useCallback((saved: Flag) => {
    setFlags(prev =>
      prev.some(f => f.key === saved.key)
        ? prev.map(f => (f.key === saved.key ? saved : f))
        : [...prev, saved],
    )
  }, [])

  const handleSaved = useCallback((saved: Flag) => {
    upsertFlag(saved)
    closePanel()
  }, [upsertFlag, closePanel])

  const handleFlagChanged = useCallback((changed: Flag) => {
    // Archiving from inside the panel removes the row from the default view,
    // matching what the list-row archive button does.
    setFlags(prev =>
      !showArchived && changed.archived_at
        ? prev.filter(f => f.key !== changed.key)
        : prev.map(f => (f.key === changed.key ? changed : f)),
    )
  }, [showArchived])

  const filtered = flags.filter(f => {
    if (statusFilter === 'enabled' && !f.is_enabled) return false
    if (statusFilter === 'disabled' && f.is_enabled) return false
    const q = query.trim().toLowerCase()
    if (!q) return true
    return f.key.toLowerCase().includes(q) || f.description?.toLowerCase().includes(q)
  })

  return (
    <div className="w-full space-y-4">
      {promotingFlag && (
        <PromoteModal flag={promotingFlag} onClose={() => setPromotingFlag(null)} />
      )}

      <Drawer
        open={panelOpen}
        onClose={closePanel}
        title={creating ? 'New flag' : 'Edit flag'}
        subtitle={
          creating ? (
            activeEnv ? `Creating in ${activeEnv.name}` : undefined
          ) : (
            <span className="font-mono text-brand-600">{editingKey}</span>
          )
        }
      >
        {/* Keyed so switching between flags (or create ↔ edit) remounts the
            form rather than reusing another flag's field state. */}
        <FlagForm
          key={creating ? '__new__' : editingKey}
          flagKey={creating ? undefined : (editingKey ?? undefined)}
          onSaved={handleSaved}
          onFlagChanged={handleFlagChanged}
          onCancel={closePanel}
        />
      </Drawer>

      {/* Toolbar */}
      <div className="flex items-center gap-3">
        <div className="relative flex-1 max-w-sm">
          <Search className="absolute left-3.5 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400 pointer-events-none" />
          <input
            ref={searchRef}
            type="search"
            placeholder="Search flags… (press /)"
            value={query}
            onChange={e => setQuery(e.target.value)}
            className="w-full bg-white border border-gray-100 rounded-xl pl-10 pr-4 py-2.5 text-sm text-gray-900 placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-brand-500/20 focus:border-brand-500/40 shadow-premium transition-all"
          />
        </div>

        {/* Status filter */}
        <div className="flex gap-0.5 p-0.5 bg-gray-100 rounded-xl text-xs font-semibold">
          {(['all', 'enabled', 'disabled'] as StatusFilter[]).map(s => (
            <button
              key={s}
              onClick={() => setStatusFilter(s)}
              className={`px-3 py-1.5 rounded-lg capitalize transition-all ${
                statusFilter === s ? 'bg-white text-gray-900 shadow-sm' : 'text-gray-500 hover:text-gray-700'
              }`}
            >
              {s}
            </button>
          ))}
        </div>

        <button
          onClick={() => setShowArchived(v => !v)}
          className={`flex items-center gap-1.5 px-3 py-1.5 rounded-xl text-xs font-semibold transition-all ${
            showArchived ? 'bg-gray-800 text-white' : 'bg-gray-100 text-gray-500 hover:text-gray-700'
          }`}
        >
          <Archive className="w-3.5 h-3.5" /> {showArchived ? 'Hide archived' : 'Show archived'}
        </button>

        {/* Active env badge */}
        {activeEnv && (
          <div className="flex items-center gap-1.5 px-3 py-1.5 bg-white border border-gray-100 rounded-xl text-xs font-semibold text-gray-600 shadow-sm">
            <div className="w-2 h-2 rounded-full" style={{ backgroundColor: activeEnv.color }} />
            {activeEnv.name}
          </div>
        )}

        <div className="flex-1" />
        {error && (
          <button
            onClick={() => { setLoading(true); void load() }}
            className="flex items-center gap-1.5 text-sm text-gray-500 hover:text-gray-800 transition-colors"
          >
            <RotateCcw className="w-3.5 h-3.5" /> Retry
          </button>
        )}
        <button
          onClick={openNew}
          className="flex items-center gap-2 px-5 py-2.5 bg-brand-600 hover:bg-brand-700 text-white text-sm font-bold rounded-xl transition-all shadow-lg shadow-brand-200 hover:shadow-brand-300 hover:-translate-y-0.5"
        >
          <Plus className="w-4 h-4" /> New flag
        </button>
      </div>

      {/* Table card */}
      <div className="premium-card shadow-premium-lg border-none bg-white">
        {loading ? (
          <div className="flex items-center justify-center h-40 text-gray-400 text-sm">Loading…</div>
        ) : error ? (
          <div className="flex items-center justify-center h-40 text-red-500 text-sm">{error}</div>
        ) : filtered.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-40 gap-3">
            {query ? (
              <p className="text-gray-400 text-sm">No flags match "<span className="text-gray-700">{query}</span>"</p>
            ) : (
              <>
                <p className="text-gray-400 text-sm">No flags in {activeEnv?.name ?? 'this environment'} yet.</p>
                <button
                  onClick={openNew}
                  className="flex items-center gap-1.5 text-sm text-brand-600 hover:text-brand-700 font-medium transition-colors"
                >
                  <Plus className="w-3.5 h-3.5" /> Create your first flag
                </button>
              </>
            )}
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-50 bg-gray-50/50">
                  <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Key</th>
                  <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden md:table-cell">Description</th>
                  <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Rollout</th>
                  <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden sm:table-cell">Rules</th>
                  <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Status</th>
                  <th className="px-8 py-4" />
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-50/50">
                {filtered.map(flag => (
                  <tr
                    key={flag.key}
                    className={`group hover:bg-brand-50/20 transition-all ${flag.archived_at ? 'opacity-50' : ''}`}
                  >
                    <td className="px-8 py-5">
                      <span className="font-mono text-brand-600 font-semibold text-sm">{flag.key}</span>
                      <TypeBadge flagType={flag.flag_type} />
                      {flag.archived_at && (
                        <span className="inline-flex items-center gap-1 ml-2 px-1.5 py-0.5 rounded text-[9px] font-bold uppercase tracking-wide bg-gray-100 text-gray-500 ring-1 ring-gray-200">
                          <Archive className="w-2.5 h-2.5" /> Archived
                        </span>
                      )}
                      {flag.tags && flag.tags.length > 0 && (
                        <div className="flex flex-wrap gap-1 mt-1">
                          {flag.tags.map(t => (
                            <span key={t} className="px-1.5 py-0.5 bg-gray-50 text-gray-500 rounded text-[9px] font-medium">
                              {t}
                            </span>
                          ))}
                        </div>
                      )}
                    </td>
                    <td className="px-8 py-5 hidden md:table-cell">
                      <span className="text-gray-500 truncate max-w-xs block text-xs">
                        {flag.description ?? <span className="text-gray-300 italic">—</span>}
                      </span>
                    </td>
                    <td className="px-8 py-5 text-gray-900 font-medium font-mono text-xs">
                      {flag.rollout_percentage != null ? `${flag.rollout_percentage}%` : '100%'}
                    </td>
                    <td className="px-8 py-5 hidden sm:table-cell">
                      {flag.rules.length > 0 ? (
                        <span className="inline-flex items-center px-2.5 py-0.5 rounded-full text-[10px] font-bold uppercase tracking-wide bg-amber-50 text-amber-700 ring-1 ring-amber-100">
                          {flag.rules.length} rule{flag.rules.length !== 1 ? 's' : ''}
                        </span>
                      ) : (
                        <span className="text-gray-300">—</span>
                      )}
                    </td>
                    <td className="px-8 py-5">
                      <Toggle enabled={flag.is_enabled} onToggle={() => void toggleEnabled(flag)} />
                    </td>
                    <td className="px-8 py-5">
                      <div className="flex items-center gap-1 justify-end opacity-0 group-hover:opacity-100 transition-all">
                        <CopyKeyButton flagKey={flag.key} />
                        <button
                          onClick={() => setPromotingFlag(flag)}
                          className="p-1.5 rounded-md text-gray-400 hover:text-indigo-600 hover:bg-indigo-50 transition-colors"
                          aria-label="Promote flag"
                          title="Promote to another environment"
                        >
                          <ArrowUpRight className="w-3.5 h-3.5" />
                        </button>
                        <button
                          onClick={() => openEdit(flag.key)}
                          className="p-1.5 rounded-md text-gray-400 hover:text-gray-700 hover:bg-gray-100 transition-colors"
                          aria-label="Edit flag"
                        >
                          <Pencil className="w-3.5 h-3.5" />
                        </button>
                        <button
                          onClick={() => void toggleArchived(flag)}
                          className="p-1.5 rounded-md text-gray-400 hover:text-gray-700 hover:bg-gray-100 transition-colors"
                          aria-label={flag.archived_at ? 'Unarchive flag' : 'Archive flag'}
                          title={flag.archived_at ? 'Unarchive' : 'Archive'}
                        >
                          {flag.archived_at ? <ArchiveRestore className="w-3.5 h-3.5" /> : <Archive className="w-3.5 h-3.5" />}
                        </button>
                        <button
                          onClick={() => void handleDelete(flag.key)}
                          className="p-1.5 rounded-md text-gray-400 hover:text-red-500 hover:bg-red-50 transition-colors"
                          aria-label="Delete flag"
                        >
                          <Trash2 className="w-3.5 h-3.5" />
                        </button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}

        {!loading && !error && filtered.length > 0 && (
          <div className="px-6 py-3 border-t border-gray-100 text-xs text-gray-400 bg-gray-50">
            {filtered.length} of {flags.length} flag{flags.length !== 1 ? 's' : ''}
            {query && ` matching "${query}"`}
            {statusFilter !== 'all' && ` · ${statusFilter} only`}
            {activeEnv && ` in ${activeEnv.name}`}
          </div>
        )}
      </div>
    </div>
  )
}
