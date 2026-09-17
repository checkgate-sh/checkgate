import { useEffect, useState, useCallback } from 'react'
import {
  FlaskConical, Plus, Trash2, ChevronDown, ChevronRight,
  Trophy, Play, Pause, CheckCircle2, X,
} from 'lucide-react'
import { api, experimentsApi, eventsApi } from '../api'
import type {
  Experiment, ExperimentResults, ExperimentStatus, EventKeyInfo, Flag, VariantResult,
} from '../types'
import { useEnvironment } from '../context/EnvironmentContext'
import { useAuth } from '../context/AuthContext'

function pct(x: number) {
  return `${(x * 100).toFixed(2)}%`
}

function StatusBadge({ status }: { status: ExperimentStatus }) {
  const map: Record<ExperimentStatus, string> = {
    running: 'bg-emerald-50 text-emerald-700 ring-emerald-200',
    paused: 'bg-amber-50 text-amber-700 ring-amber-200',
    completed: 'bg-gray-100 text-gray-500 ring-gray-200',
  }
  return (
    <span className={`inline-flex items-center px-2 py-0.5 rounded-full text-[10px] font-bold uppercase tracking-wide ring-1 ${map[status]}`}>
      {status}
    </span>
  )
}

// ---------------------------------------------------------------------------
// Results panel
// ---------------------------------------------------------------------------

function UpliftCell({ r }: { r: VariantResult }) {
  if (r.is_control) return <span className="text-gray-400 text-xs">baseline</span>
  if (r.uplift == null) return <span className="text-gray-300">—</span>
  const positive = r.uplift >= 0
  return (
    <span className={`font-bold tabular-nums ${positive ? 'text-emerald-600' : 'text-rose-600'}`}>
      {positive ? '+' : ''}{(r.uplift * 100).toFixed(1)}%
    </span>
  )
}

function SignificanceCell({ r }: { r: VariantResult }) {
  if (r.is_control || r.p_value == null) return <span className="text-gray-300">—</span>
  return (
    <div className="flex items-center gap-2">
      <span className="text-xs text-gray-500 tabular-nums">p={r.p_value < 0.001 ? '<0.001' : r.p_value.toFixed(3)}</span>
      {r.significant ? (
        <span className="inline-flex items-center gap-1 text-[10px] font-bold uppercase tracking-wide text-emerald-700">
          <CheckCircle2 className="w-3 h-3" /> Sig.
        </span>
      ) : (
        <span className="text-[10px] font-bold uppercase tracking-wide text-gray-400">Not sig.</span>
      )}
    </div>
  )
}

function ResultsPanel({ results }: { results: ExperimentResults }) {
  const maxRate = Math.max(...results.variants.map(v => v.conversion_rate), 0.0001)
  // A "winner" is the best non-control variant that is statistically significant.
  const winner = results.variants
    .filter(v => !v.is_control && v.significant && (v.uplift ?? 0) > 0)
    .sort((a, b) => b.conversion_rate - a.conversion_rate)[0]

  if (results.total_exposed === 0) {
    return (
      <div className="px-6 py-8 text-center text-sm text-gray-400">
        No exposed users yet. Once the flag <span className="font-mono text-gray-500">{results.experiment.flag_key}</span> is
        evaluated and the goal <span className="font-mono text-gray-500">{results.experiment.goal_event_key}</span> is tracked,
        results appear here.
      </div>
    )
  }

  return (
    <div className="px-6 py-5 space-y-4">
      <div className="flex flex-wrap gap-6 text-sm">
        <div>
          <span className="text-gray-400 text-xs uppercase tracking-wider font-bold">Exposed</span>
          <p className="font-display font-bold text-gray-900 text-lg leading-tight">{results.total_exposed.toLocaleString()}</p>
        </div>
        <div>
          <span className="text-gray-400 text-xs uppercase tracking-wider font-bold">Converted</span>
          <p className="font-display font-bold text-gray-900 text-lg leading-tight">{results.total_converted.toLocaleString()}</p>
        </div>
        <div>
          <span className="text-gray-400 text-xs uppercase tracking-wider font-bold">Control</span>
          <p className="font-mono font-semibold text-gray-700 text-sm leading-tight mt-1">{results.control_variant ?? '—'}</p>
        </div>
        {winner && (
          <div className="flex items-center gap-2 ml-auto px-3 py-1.5 bg-emerald-50 rounded-xl ring-1 ring-emerald-100">
            <Trophy className="w-4 h-4 text-emerald-600" />
            <span className="text-sm font-semibold text-emerald-700">
              Winner: <span className="font-mono">{winner.variant}</span>
            </span>
          </div>
        )}
      </div>

      <div className="overflow-x-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-50 bg-gray-50/50">
              <th className="text-left px-4 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Variant</th>
              <th className="text-left px-4 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Conversion rate</th>
              <th className="text-left px-4 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden sm:table-cell">Converted / Exposed</th>
              <th className="text-left px-4 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Uplift</th>
              <th className="text-left px-4 py-3 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Significance</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-50/50">
            {results.variants.map(v => (
              <tr key={v.variant} className="hover:bg-brand-50/20">
                <td className="px-4 py-3">
                  <div className="flex items-center gap-2">
                    <span className="font-mono font-semibold text-gray-800">{v.variant}</span>
                    {v.is_control && (
                      <span className="text-[9px] font-bold uppercase tracking-wide text-gray-400 bg-gray-100 px-1.5 py-0.5 rounded">control</span>
                    )}
                  </div>
                </td>
                <td className="px-4 py-3">
                  <div className="flex items-center gap-3">
                    <span className="font-bold text-gray-900 tabular-nums w-16">{pct(v.conversion_rate)}</span>
                    <div className="flex-1 max-w-[140px] h-2 bg-gray-100 rounded-full overflow-hidden">
                      <div
                        className={`h-full rounded-full ${v.is_control ? 'bg-gray-400' : 'bg-brand-500'}`}
                        style={{ width: `${(v.conversion_rate / maxRate) * 100}%` }}
                      />
                    </div>
                  </div>
                </td>
                <td className="px-4 py-3 hidden sm:table-cell text-gray-500 tabular-nums">
                  {v.converted.toLocaleString()} / {v.exposed.toLocaleString()}
                </td>
                <td className="px-4 py-3"><UpliftCell r={v} /></td>
                <td className="px-4 py-3"><SignificanceCell r={v} /></td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="text-[11px] text-gray-400">
        Each user enters the experiment at their first exposure to the flag and is bucketed into the variant they saw
        then; they count as converted only if they fired the goal event at or after that exposure. Significance is a
        two-proportion z-test vs. the control (95%).
      </p>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Create form
// ---------------------------------------------------------------------------

function CreateForm({
  flags, eventKeys, onCreate, onCancel,
}: {
  flags: Flag[]
  eventKeys: EventKeyInfo[]
  onCreate: (data: {
    key: string; name: string; description?: string
    flag_key: string; goal_event_key: string; control_variant?: string | null
  }) => Promise<void>
  onCancel: () => void
}) {
  const [name, setName] = useState('')
  const [key, setKey] = useState('')
  const [flagKey, setFlagKey] = useState(flags[0]?.key ?? '')
  const [goal, setGoal] = useState('')
  const [control, setControl] = useState('')
  const [description, setDescription] = useState('')
  const [saving, setSaving] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  // Suggest a key derived from the name until the user edits it themselves.
  const [keyTouched, setKeyTouched] = useState(false)
  function updateName(v: string) {
    setName(v)
    if (!keyTouched) setKey(v.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, ''))
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setErr(null)
    if (!name.trim() || !key.trim() || !flagKey || !goal.trim()) {
      setErr('Name, key, flag, and goal event are required.')
      return
    }
    setSaving(true)
    try {
      await onCreate({
        key: key.trim(),
        name: name.trim(),
        description: description.trim() || undefined,
        flag_key: flagKey,
        goal_event_key: goal.trim(),
        control_variant: control.trim() || null,
      })
    } catch (e2) {
      setErr(e2 instanceof Error ? e2.message : 'Failed to create experiment')
    } finally {
      setSaving(false)
    }
  }

  const inputCls = 'w-full px-3 py-2 rounded-xl border border-gray-200 bg-white text-sm focus:outline-none focus:ring-2 focus:ring-brand-200'
  const labelCls = 'block text-[11px] font-bold uppercase tracking-wider text-gray-400 mb-1'

  return (
    <form onSubmit={submit} className="premium-card bg-white p-6 space-y-4">
      <div className="flex items-center justify-between">
        <h2 className="font-display font-bold text-gray-900">New experiment</h2>
        <button type="button" onClick={onCancel} className="text-gray-400 hover:text-gray-600"><X className="w-4 h-4" /></button>
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
        <div>
          <label className={labelCls}>Name</label>
          <input className={inputCls} value={name} onChange={e => updateName(e.target.value)} placeholder="Checkout button color" />
        </div>
        <div>
          <label className={labelCls}>Key</label>
          <input className={`${inputCls} font-mono`} value={key} onChange={e => { setKeyTouched(true); setKey(e.target.value) }} placeholder="checkout-button-color" />
        </div>
        <div>
          <label className={labelCls}>Flag (variant source)</label>
          <select className={`${inputCls} font-mono`} value={flagKey} onChange={e => setFlagKey(e.target.value)}>
            {flags.length === 0 && <option value="">No flags</option>}
            {flags.map(f => <option key={f.key} value={f.key}>{f.key}</option>)}
          </select>
        </div>
        <div>
          <label className={labelCls}>Goal event</label>
          <input
            className={`${inputCls} font-mono`}
            value={goal}
            onChange={e => setGoal(e.target.value)}
            placeholder="checkout_complete"
            list="event-keys"
          />
          <datalist id="event-keys">
            {eventKeys.map(k => <option key={k.event_key} value={k.event_key} />)}
          </datalist>
        </div>
        <div>
          <label className={labelCls}>Control variant <span className="text-gray-300 normal-case">(optional)</span></label>
          <input className={`${inputCls} font-mono`} value={control} onChange={e => setControl(e.target.value)} placeholder="auto (highest exposure)" />
        </div>
        <div>
          <label className={labelCls}>Description <span className="text-gray-300 normal-case">(optional)</span></label>
          <input className={inputCls} value={description} onChange={e => setDescription(e.target.value)} placeholder="What are we testing?" />
        </div>
      </div>
      {err && <p className="text-sm text-rose-600">{err}</p>}
      <div className="flex justify-end gap-2">
        <button type="button" onClick={onCancel} className="px-4 py-2 rounded-xl text-sm font-semibold text-gray-500 hover:bg-gray-50">Cancel</button>
        <button type="submit" disabled={saving} className="px-4 py-2 rounded-xl text-sm font-semibold bg-brand-600 text-white hover:bg-brand-700 disabled:opacity-50">
          {saving ? 'Creating…' : 'Create experiment'}
        </button>
      </div>
    </form>
  )
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function Experiments() {
  const { activeEnv } = useEnvironment()
  const { session } = useAuth()
  const canEdit = session?.user.role === 'admin' || session?.user.role === 'editor'

  const [experiments, setExperiments] = useState<Experiment[]>([])
  const [flags, setFlags] = useState<Flag[]>([])
  const [eventKeys, setEventKeys] = useState<EventKeyInfo[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)

  const [expanded, setExpanded] = useState<string | null>(null)
  const [resultsByKey, setResultsByKey] = useState<Record<string, ExperimentResults>>({})

  const load = useCallback(async () => {
    if (!activeEnv) return
    try {
      setError(null)
      const [exps, flagList, evKeys] = await Promise.all([
        experimentsApi.list(activeEnv.id),
        api.listFlags(activeEnv.id).catch(() => [] as Flag[]),
        eventsApi.keys(activeEnv.id).catch(() => [] as EventKeyInfo[]),
      ])
      setExperiments(exps)
      setFlags(flagList)
      setEventKeys(evKeys)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load experiments')
    } finally {
      setLoading(false)
    }
  }, [activeEnv])

  useEffect(() => { void load() }, [load])

  const toggle = useCallback(async (key: string) => {
    if (expanded === key) { setExpanded(null); return }
    setExpanded(key)
    if (!resultsByKey[key] && activeEnv) {
      try {
        const r = await experimentsApi.results(activeEnv.id, key)
        setResultsByKey(prev => ({ ...prev, [key]: r }))
      } catch { /* surfaced inline as "no results" */ }
    }
  }, [expanded, resultsByKey, activeEnv])

  async function create(data: Parameters<typeof experimentsApi.create>[1]) {
    if (!activeEnv) return
    await experimentsApi.create(activeEnv.id, data)
    setCreating(false)
    await load()
  }

  async function setStatus(exp: Experiment, status: ExperimentStatus) {
    if (!activeEnv) return
    await experimentsApi.patch(activeEnv.id, exp.key, { status })
    await load()
  }

  async function remove(exp: Experiment) {
    if (!activeEnv) return
    if (!window.confirm(`Delete experiment "${exp.name}"? This does not touch the flag or its data.`)) return
    await experimentsApi.delete(activeEnv.id, exp.key)
    setResultsByKey(prev => { const n = { ...prev }; delete n[exp.key]; return n })
    await load()
  }

  return (
    <div className="w-full space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <div>
          <h1 className="text-xl font-display font-bold text-gray-900">Experiments</h1>
          <p className="text-sm text-gray-400 mt-0.5">A/B test flag variants against a conversion goal.</p>
        </div>
        {canEdit && !creating && (
          <button
            onClick={() => setCreating(true)}
            className="flex items-center gap-1.5 px-4 py-2 rounded-xl text-sm font-semibold bg-brand-600 text-white hover:bg-brand-700"
          >
            <Plus className="w-4 h-4" /> New experiment
          </button>
        )}
      </div>

      {creating && (
        <CreateForm flags={flags} eventKeys={eventKeys} onCreate={create} onCancel={() => setCreating(false)} />
      )}

      {loading ? (
        <div className="flex items-center justify-center h-40 text-gray-400 text-sm">Loading…</div>
      ) : error ? (
        <div className="flex items-center justify-center h-40 text-red-500 text-sm">{error}</div>
      ) : experiments.length === 0 && !creating ? (
        <div className="premium-card bg-white flex flex-col items-center justify-center h-48 gap-2">
          <FlaskConical className="w-8 h-8 text-gray-300" />
          <p className="text-gray-400 text-sm">No experiments yet.</p>
          {canEdit && (
            <button onClick={() => setCreating(true)} className="flex items-center gap-1.5 text-sm text-brand-600 hover:text-brand-700 font-medium">
              <Plus className="w-3.5 h-3.5" /> Create your first experiment
            </button>
          )}
        </div>
      ) : (
        <div className="space-y-3">
          {experiments.map(exp => {
            const isOpen = expanded === exp.key
            return (
              <div key={exp.id} className="premium-card bg-white overflow-hidden">
                <div className="flex items-center gap-4 px-6 py-4">
                  <button onClick={() => toggle(exp.key)} className="flex items-center gap-3 flex-1 min-w-0 text-left">
                    {isOpen ? <ChevronDown className="w-4 h-4 text-gray-400 shrink-0" /> : <ChevronRight className="w-4 h-4 text-gray-400 shrink-0" />}
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="font-display font-bold text-gray-900 truncate">{exp.name}</span>
                        <StatusBadge status={exp.status} />
                      </div>
                      <p className="text-xs text-gray-400 mt-0.5 truncate">
                        <span className="font-mono">{exp.flag_key}</span>
                        <span className="mx-1.5">→</span>
                        <span className="font-mono">{exp.goal_event_key}</span>
                      </p>
                    </div>
                  </button>
                  {canEdit && (
                    <div className="flex items-center gap-1 shrink-0">
                      {exp.status !== 'running' && (
                        <button onClick={() => setStatus(exp, 'running')} title="Resume" className="p-2 rounded-lg text-gray-400 hover:text-brand-600 hover:bg-brand-50"><Play className="w-4 h-4" /></button>
                      )}
                      {exp.status === 'running' && (
                        <button onClick={() => setStatus(exp, 'paused')} title="Pause" className="p-2 rounded-lg text-gray-400 hover:text-amber-600 hover:bg-amber-50"><Pause className="w-4 h-4" /></button>
                      )}
                      {exp.status !== 'completed' && (
                        <button onClick={() => setStatus(exp, 'completed')} title="Mark completed" className="p-2 rounded-lg text-gray-400 hover:text-gray-700 hover:bg-gray-100"><CheckCircle2 className="w-4 h-4" /></button>
                      )}
                      <button onClick={() => remove(exp)} title="Delete" className="p-2 rounded-lg text-gray-400 hover:text-rose-600 hover:bg-rose-50"><Trash2 className="w-4 h-4" /></button>
                    </div>
                  )}
                </div>
                {isOpen && (
                  <div className="border-t border-gray-50">
                    {resultsByKey[exp.key]
                      ? <ResultsPanel results={resultsByKey[exp.key]} />
                      : <div className="px-6 py-8 text-center text-sm text-gray-400">Loading results…</div>}
                  </div>
                )}
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
