import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { Archive, ArchiveRestore, Plus, Save, Trash2, X } from 'lucide-react'
import { api, scheduledApi, segmentsApi } from '../api'
import type { Flag, FlagType, FlagValue, Prerequisite, ScheduledChange, Segment, TargetingRule, WeightedVariant } from '../types'
import RuleEditor from './RuleEditor'
import { useEnvironment } from '../context/EnvironmentContext'

const EMPTY_FLAG: Flag = {
  key: '',
  is_enabled: true,
  rollout_percentage: null,
  description: null,
  rules: [],
  flag_type: 'boolean',
  default_value: null,
  disabled_value: null,
  variants: [],
  prerequisites: [],
  tags: [],
  owner_email: null,
}

function SectionCard({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="premium-card shadow-premium-lg border-none">
      <div className="px-5 py-3.5 border-b border-gray-50 bg-white">
        <h3 className="text-gray-900 font-display font-bold text-sm tracking-tight">{title}</h3>
      </div>
      <div className="p-5">{children}</div>
    </div>
  )
}

function Toggle({ enabled, onToggle }: { enabled: boolean; onToggle: () => void }) {
  return (
    <button
      type="button"
      onClick={onToggle}
      className={`relative inline-flex h-6 w-11 items-center rounded-full transition-all duration-300 focus:outline-none focus-visible:ring-2 focus-visible:ring-brand-500/20 ${
        enabled ? 'bg-brand-600 shadow-md shadow-brand-200' : 'bg-gray-200'
      }`}
      aria-label={enabled ? 'Disable flag' : 'Enable flag'}
    >
      <span
        className={`inline-block h-4.5 w-4.5 transform rounded-full bg-white shadow-sm transition-transform duration-300 ${
          enabled ? 'translate-x-6' : 'translate-x-0.5'
        }`}
      />
    </button>
  )
}

const inputClass =
  'w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium disabled:opacity-50 disabled:cursor-not-allowed'

const selectClass =
  'w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium appearance-none disabled:opacity-50 disabled:cursor-not-allowed'

const FLAG_TYPES: { value: FlagType; label: string; description: string }[] = [
  { value: 'boolean', label: 'Boolean', description: 'On / off toggle' },
  { value: 'string', label: 'String', description: 'Text value (e.g. "variant-a", "dark")' },
  { value: 'integer', label: 'Integer', description: 'Whole number (e.g. 42, 100)' },
  { value: 'json', label: 'JSON', description: 'Arbitrary object or array' },
]

function parseValue(raw: string, flagType: FlagType): FlagValue {
  if (flagType === 'boolean') return raw === 'true'
  if (raw.trim() === '') return null
  if (flagType === 'integer') {
    const n = parseInt(raw, 10)
    return isNaN(n) ? null : n
  }
  if (flagType === 'json') {
    try { return JSON.parse(raw) } catch { return null }
  }
  return raw
}

function valueToString(v: FlagValue): string {
  if (v == null) return ''
  if (typeof v === 'object') return JSON.stringify(v, null, 2)
  return String(v)
}

/**
 * A sensible non-null starting value per type. Used when a field transitions from
 * "unset" to "set" (e.g. checking "require a specific value") — `null` would be
 * indistinguishable from "unset" once round-tripped through the server, since a
 * JSON `null` deserializes to `Option::None` there, same as an absent field.
 */
function defaultValueForType(flagType: FlagType): FlagValue {
  switch (flagType) {
    case 'boolean': return true
    case 'integer': return 0
    case 'json': return {}
    default: return ''
  }
}

function ValueInput({
  label,
  hint,
  value,
  flagType,
  onChange,
}: {
  label: string
  hint?: string
  value: FlagValue
  flagType: FlagType
  onChange: (v: FlagValue) => void
}) {
  const raw = valueToString(value)
  const base = `${inputClass}`

  return (
    <div>
      <label className="block text-sm font-medium text-gray-700 mb-1.5">{label}</label>
      {flagType === 'boolean' ? (
        <select
          value={raw === 'false' ? 'false' : 'true'}
          onChange={e => onChange(parseValue(e.target.value, flagType))}
          className={selectClass}
        >
          <option value="true">true</option>
          <option value="false">false</option>
        </select>
      ) : flagType === 'json' ? (
        <textarea
          rows={3}
          value={raw}
          onChange={e => onChange(parseValue(e.target.value, flagType))}
          placeholder='{"key": "value"}'
          className={`${base} font-mono text-xs resize-none`}
        />
      ) : (
        <input
          type={flagType === 'integer' ? 'number' : 'text'}
          value={raw}
          onChange={e => onChange(parseValue(e.target.value, flagType))}
          placeholder={flagType === 'integer' ? '0' : 'value'}
          className={base}
        />
      )}
      {hint && <p className="mt-1.5 text-xs text-gray-400">{hint}</p>}
    </div>
  )
}

function WeightedVariantsEditor({
  variants,
  flagType,
  onChange,
}: {
  variants: WeightedVariant[]
  flagType: FlagType
  onChange: (variants: WeightedVariant[]) => void
}) {
  const totalWeight = variants.reduce((sum, v) => sum + (v.weight || 0), 0)

  function updateVariant(i: number, patch: Partial<WeightedVariant>) {
    onChange(variants.map((v, idx) => (idx === i ? { ...v, ...patch } : v)))
  }

  function removeVariant(i: number) {
    onChange(variants.filter((_, idx) => idx !== i))
  }

  function addVariant() {
    onChange([...variants, { value: null, weight: variants.length === 0 ? 100 : 0 }])
  }

  return (
    <div className="space-y-3">
      {variants.length === 0 && (
        <p className="text-xs text-gray-400">
          No weighted variants configured — the default value above is always returned.
        </p>
      )}
      {variants.map((v, i) => {
        const pct = totalWeight > 0 ? Math.round((v.weight / totalWeight) * 1000) / 10 : 0
        return (
          <div key={i} className="flex items-start gap-2">
            <div className="flex-1">
              <ValueInput
                label={`Variant ${i + 1}`}
                value={v.value}
                flagType={flagType}
                onChange={val => updateVariant(i, { value: val })}
              />
            </div>
            <div className="w-24 shrink-0">
              <label className="block text-sm font-medium text-gray-700 mb-1.5">Weight</label>
              <input
                type="number"
                min={0}
                value={v.weight}
                onChange={e => updateVariant(i, { weight: parseInt(e.target.value, 10) || 0 })}
                className={inputClass}
              />
              <p className="mt-1.5 text-xs text-gray-400">{pct}%</p>
            </div>
            <button
              type="button"
              onClick={() => removeVariant(i)}
              className="mt-8 text-red-400 hover:text-red-600 shrink-0"
              aria-label="Remove variant"
            >
              <Trash2 className="w-4 h-4" />
            </button>
          </div>
        )
      })}
      <button
        type="button"
        onClick={addVariant}
        className="inline-flex items-center gap-1.5 text-sm font-medium text-brand-600 hover:text-brand-700"
      >
        <Plus className="w-3.5 h-3.5" /> Add variant
      </button>
    </div>
  )
}

function PrerequisitesEditor({
  prerequisites,
  candidates,
  onChange,
}: {
  prerequisites: Prerequisite[]
  /** Other flags in this environment that could be used as a prerequisite. */
  candidates: Flag[]
  onChange: (prerequisites: Prerequisite[]) => void
}) {
  function updatePrereq(i: number, patch: Partial<Prerequisite>) {
    onChange(prerequisites.map((p, idx) => (idx === i ? { ...p, ...patch } : p)))
  }

  function removePrereq(i: number) {
    onChange(prerequisites.filter((_, idx) => idx !== i))
  }

  function addPrereq() {
    if (candidates.length === 0) return
    onChange([...prerequisites, { flag_key: candidates[0].key }])
  }

  return (
    <div className="space-y-3">
      {candidates.length === 0 && prerequisites.length === 0 && (
        <p className="text-xs text-gray-400">
          No other flags exist in this environment yet to depend on.
        </p>
      )}
      {prerequisites.length === 0 && candidates.length > 0 && (
        <p className="text-xs text-gray-400">
          No prerequisites configured — this flag evaluates independently.
        </p>
      )}
      {prerequisites.map((p, i) => {
        const candidate = candidates.find(c => c.key === p.flag_key)
        const candidateType = candidate?.flag_type ?? 'boolean'
        const requiresValue = p.required_value !== undefined
        return (
          <div key={i} className="flex items-start gap-2">
            <div className="flex-1 space-y-2">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1.5">Requires flag</label>
                <select
                  value={p.flag_key}
                  onChange={e => updatePrereq(i, { flag_key: e.target.value })}
                  className={selectClass}
                >
                  {!candidates.some(c => c.key === p.flag_key) && (
                    <option value={p.flag_key}>{p.flag_key} (not found)</option>
                  )}
                  {candidates.map(c => (
                    <option key={c.key} value={c.key}>{c.key} ({c.flag_type ?? 'boolean'})</option>
                  ))}
                </select>
              </div>
              <label className="flex items-center gap-2 text-xs text-gray-500">
                <input
                  type="checkbox"
                  checked={requiresValue}
                  onChange={e => updatePrereq(i, {
                    required_value: e.target.checked ? defaultValueForType(candidateType) : undefined,
                  })}
                />
                Require a specific resolved value (instead of just "enabled")
              </label>
              {requiresValue && (
                <ValueInput
                  label="Required value"
                  value={p.required_value ?? null}
                  flagType={candidateType}
                  onChange={v => updatePrereq(i, { required_value: v })}
                />
              )}
            </div>
            <button
              type="button"
              onClick={() => removePrereq(i)}
              className="mt-8 text-red-400 hover:text-red-600 shrink-0"
              aria-label="Remove prerequisite"
            >
              <Trash2 className="w-4 h-4" />
            </button>
          </div>
        )
      })}
      {candidates.length > 0 && (
        <button
          type="button"
          onClick={addPrereq}
          className="inline-flex items-center gap-1.5 text-sm font-medium text-brand-600 hover:text-brand-700"
        >
          <Plus className="w-3.5 h-3.5" /> Add prerequisite
        </button>
      )}
    </div>
  )
}

function TagsInput({ tags, onChange }: { tags: string[]; onChange: (tags: string[]) => void }) {
  const [draft, setDraft] = useState('')

  function addTag() {
    const t = draft.trim()
    if (t && !tags.includes(t)) onChange([...tags, t])
    setDraft('')
  }

  return (
    <div>
      <label className="block text-sm font-medium text-gray-700 mb-1.5">Tags</label>
      {tags.length > 0 && (
        <div className="flex flex-wrap gap-1.5 mb-2">
          {tags.map(t => (
            <span
              key={t}
              className="inline-flex items-center gap-1 pl-2 pr-1 py-0.5 bg-brand-50 text-brand-700 rounded text-xs font-medium"
            >
              {t}
              <button
                type="button"
                onClick={() => onChange(tags.filter(x => x !== t))}
                className="hover:text-brand-900"
                aria-label={`Remove tag ${t}`}
              >
                <X className="w-3 h-3" />
              </button>
            </span>
          ))}
        </div>
      )}
      <input
        type="text"
        value={draft}
        onChange={e => setDraft(e.target.value)}
        onKeyDown={e => {
          if (e.key === 'Enter' || e.key === ',') {
            e.preventDefault()
            addTag()
          }
        }}
        onBlur={addTag}
        placeholder="Add a tag and press Enter"
        className={inputClass}
      />
    </div>
  )
}

/**
 * Create/edit form for a single flag, laid out as one scrolling column with a
 * pinned action bar — sized for the slide-over panel rather than a full page.
 *
 * Fills its parent as a flex column, so it expects a `min-h-0` flex container.
 */
export default function FlagForm({
  flagKey,
  onSaved,
  onFlagChanged,
  onCancel,
}: {
  /** Key of the flag to edit; omit to create a new one. */
  readonly flagKey?: string
  /** The save landed and the panel is done — not called for queued approvals. */
  readonly onSaved: (flag: Flag, mode: 'created' | 'updated') => void
  /** A side-channel write (archive/unarchive) landed; the panel stays open. */
  readonly onFlagChanged?: (flag: Flag) => void
  readonly onCancel: () => void
}) {
  const isEdit = Boolean(flagKey)
  const { activeEnv } = useEnvironment()

  const [flag, setFlag] = useState<Flag>(EMPTY_FLAG)
  const [rolloutInput, setRolloutInput] = useState('')
  const [loading, setLoading] = useState(isEdit)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [pendingApproval, setPendingApproval] = useState(false)
  const [segments, setSegments] = useState<Segment[]>([])
  const [otherFlags, setOtherFlags] = useState<Flag[]>([])
  const [scheduledChanges, setScheduledChanges] = useState<ScheduledChange[]>([])
  const [scheduleAt, setScheduleAt] = useState('')
  const [scheduleAction, setScheduleAction] = useState<'enable' | 'disable'>('enable')
  const [scheduleSaving, setScheduleSaving] = useState(false)
  const [archiving, setArchiving] = useState(false)

  // The panel is reused across flags without unmounting, so every piece of
  // per-flag state has to be reset when the target changes — otherwise the
  // previous flag's values bleed into the next one.
  useEffect(() => {
    setFlag(EMPTY_FLAG)
    setRolloutInput('')
    setError(null)
    setPendingApproval(false)
    setScheduledChanges([])
    setScheduleAt('')
    setLoading(Boolean(flagKey))
  }, [flagKey, activeEnv])

  useEffect(() => {
    if (!activeEnv) return
    segmentsApi.list(activeEnv.id).then(setSegments).catch(() => {/* non-fatal */})
    // Candidates for the Prerequisites picker — exclude the flag being edited
    // itself, since a self-referencing prerequisite is a trivial cycle.
    api.listFlags(activeEnv.id)
      .then(list => setOtherFlags(list.filter(f => f.key !== flagKey)))
      .catch(() => {/* non-fatal */})
  }, [activeEnv, flagKey])

  useEffect(() => {
    if (!flagKey || !activeEnv) return
    let cancelled = false
    api.getFlag(activeEnv.id, flagKey)
      .then(f => {
        if (cancelled) return
        setFlag(f)
        setRolloutInput(f.rollout_percentage != null ? String(f.rollout_percentage) : '')
      })
      .catch(e => {
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load flag')
      })
      .finally(() => { if (!cancelled) setLoading(false) })
    scheduledApi.listForFlag(activeEnv.id, flagKey)
      .then(cs => { if (!cancelled) setScheduledChanges(cs) })
      .catch(() => {/* non-fatal */})
    return () => { cancelled = true }
  }, [flagKey, activeEnv])

  async function handleSchedule(e: React.FormEvent) {
    e.preventDefault()
    if (!activeEnv || !flagKey) return
    setScheduleSaving(true)
    try {
      const patch: Record<string, unknown> = { is_enabled: scheduleAction === 'enable' }
      const created = await scheduledApi.create(activeEnv.id, flagKey, {
        scheduled_at: new Date(scheduleAt).toISOString(),
        patch,
      })
      setScheduledChanges(prev => [...prev, created])
      setScheduleAt('')
    } catch (err) {
      alert(err instanceof Error ? err.message : String(err))
    } finally {
      setScheduleSaving(false)
    }
  }

  async function cancelScheduledChange(id: string) {
    if (!activeEnv || !confirm('Cancel this scheduled change?')) return
    try {
      await scheduledApi.delete(activeEnv.id, id)
      setScheduledChanges(prev => prev.filter(c => c.id !== id))
    } catch (err) {
      alert(err instanceof Error ? err.message : String(err))
    }
  }

  async function handleArchiveToggle() {
    if (!activeEnv || !flagKey) return
    const archiving_ = !flag.archived_at
    if (archiving_ && !confirm(`Archive "${flagKey}"? It will be hidden from the flag list, but keeps evaluating exactly as before.`)) return
    setArchiving(true)
    try {
      const updated = archiving_
        ? await api.archiveFlag(activeEnv.id, flagKey)
        : await api.unarchiveFlag(activeEnv.id, flagKey)
      setFlag(updated)
      // Surface the archive/unarchive to the list behind the panel, which
      // filters on exactly this field. The panel stays open — archiving is not
      // "done editing".
      onFlagChanged?.(updated)
    } catch (err) {
      alert(err instanceof Error ? err.message : String(err))
    } finally {
      setArchiving(false)
    }
  }

  function setField<K extends keyof Flag>(field: K, value: Flag[K]) {
    setFlag(prev => ({ ...prev, [field]: value }))
  }

  function handleTypeChange(newType: FlagType) {
    setFlag(prev => ({
      ...prev,
      flag_type: newType,
      default_value: null,
      disabled_value: null,
      variants: [],
      rules: prev.rules.map(r => ({ ...r, variant: undefined })),
    }))
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    setSaving(true)
    setError(null)
    setPendingApproval(false)

    const rollout = rolloutInput.trim() === '' ? null : parseInt(rolloutInput, 10)
    if (rollout !== null && (isNaN(rollout) || rollout < 0 || rollout > 100)) {
      setError('Rollout percentage must be 0–100 or empty.')
      setSaving(false)
      return
    }

    const payload: Flag = { ...flag, rollout_percentage: rollout }

    if (!activeEnv) {
      setError('No active environment selected.')
      setSaving(false)
      return
    }

    try {
      if (isEdit && flagKey) {
        const result = await api.patchFlag(activeEnv.id, flagKey, {
          is_enabled: payload.is_enabled,
          rollout_percentage: payload.rollout_percentage,
          description: payload.description,
          rules: payload.rules,
          flag_type: payload.flag_type,
          default_value: payload.default_value,
          disabled_value: payload.disabled_value,
          variants: payload.variants,
          prerequisites: payload.prerequisites,
          tags: payload.tags,
          owner_email: payload.owner_email,
        })
        if (!result.applied) {
          // Environment requires approval — the patch was queued, not applied.
          // Keep the panel open rather than closing it as if it took effect.
          setPendingApproval(true)
          setSaving(false)
          return
        }
        onSaved(result.flag, 'updated')
      } else {
        const created = await api.createFlag(activeEnv.id, payload)
        onSaved(created, 'created')
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Save failed')
    } finally {
      setSaving(false)
    }
  }

  const flagType = flag.flag_type ?? 'boolean'
  const isVariant = flagType !== 'boolean'

  if (loading) {
    return (
      <div className="flex flex-1 items-center justify-center text-gray-400 text-sm">
        Loading…
      </div>
    )
  }

  return (
    <form onSubmit={e => void handleSubmit(e)} className="flex min-h-0 flex-1 flex-col">
      <div className="flex-1 space-y-4 overflow-y-auto px-6 py-5">
        {error && (
          <div className="p-3.5 rounded-lg bg-red-50 border border-red-200 text-red-600 text-sm">
            {error}
          </div>
        )}

        {pendingApproval && (
          <div className="p-3.5 rounded-lg bg-amber-50 border border-amber-200 text-amber-700 text-sm">
            This environment requires approval — your change was queued instead of applied.{' '}
            <Link to="/change-requests" className="font-bold underline">View change requests</Link>
          </div>
        )}

        <SectionCard title="Basic info">
          <div className="space-y-4">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1.5">
                Key <span className="text-rose-400">*</span>
              </label>
              <input
                type="text"
                required
                disabled={isEdit}
                value={flag.key}
                onChange={e => setField('key', e.target.value)}
                placeholder="e.g. dark_mode"
                className={`${inputClass} font-mono`}
              />
              {!isEdit && (
                <p className="mt-1.5 text-xs text-gray-400">
                  Immutable after creation. Use <code className="text-gray-600">snake_case</code>.
                </p>
              )}
            </div>

            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1.5">Description</label>
              <input
                type="text"
                value={flag.description ?? ''}
                onChange={e => setField('description', e.target.value || null)}
                placeholder="What does this flag control?"
                className={inputClass}
              />
            </div>

            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1.5">Type</label>
              <select
                value={flagType}
                onChange={e => handleTypeChange(e.target.value as FlagType)}
                disabled={isEdit}
                className={selectClass}
              >
                {FLAG_TYPES.map(t => (
                  <option key={t.value} value={t.value}>{t.label} — {t.description}</option>
                ))}
              </select>
              {isEdit && (
                <p className="mt-1.5 text-xs text-gray-400">
                  Flag type cannot be changed after creation.
                </p>
              )}
            </div>

            {isVariant && (
              <>
                <ValueInput
                  label="Default value"
                  hint="Returned when the flag is enabled and no targeting rule overrides it."
                  value={flag.default_value ?? null}
                  flagType={flagType}
                  onChange={v => setField('default_value', v)}
                />
                <ValueInput
                  label="Disabled value"
                  hint="Returned when the flag is disabled or the user is outside the rollout."
                  value={flag.disabled_value ?? null}
                  flagType={flagType}
                  onChange={v => setField('disabled_value', v)}
                />
              </>
            )}
          </div>
        </SectionCard>

        <SectionCard title="Rollout">
          <div className="space-y-4">
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm font-medium text-gray-700">Enabled</p>
                <p className="text-xs text-gray-400 mt-0.5">
                  {isVariant
                    ? 'When disabled, returns the disabled value.'
                    : <>When disabled, always evaluates to <code className="text-gray-600">false</code>.</>}
                </p>
              </div>
              <Toggle enabled={flag.is_enabled} onToggle={() => setField('is_enabled', !flag.is_enabled)} />
            </div>

            <div className="border-t border-gray-100 pt-4">
              <label className="block text-sm font-medium text-gray-700 mb-1.5">Rollout percentage</label>
              <div className="flex items-center gap-2">
                <input
                  type="number"
                  min={0}
                  max={100}
                  value={rolloutInput}
                  onChange={e => setRolloutInput(e.target.value)}
                  placeholder="100"
                  className="w-28 bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
                />
                <span className="text-gray-400 text-sm">%</span>
              </div>
              <p className="mt-1.5 text-xs text-gray-400">
                Leave empty for 100%. Users are bucketed deterministically by their key.
              </p>
            </div>
          </div>
        </SectionCard>

        {isVariant && (
          <SectionCard title="Weighted variants (A/B testing)">
            <p className="text-xs text-gray-400 mb-4">
              Split traffic across multiple values by weight (e.g. 60/30/10). Applies to users who
              are enabled, inside the rollout percentage above, and don't match a targeting rule.
              Weights don't need to sum to 100 — only their proportions matter. Leave empty to
              always return the default value above.
            </p>
            <WeightedVariantsEditor
              variants={flag.variants ?? []}
              flagType={flagType}
              onChange={variants => setField('variants', variants)}
            />
          </SectionCard>
        )}

        <SectionCard title="Targeting rules">
          <p className="text-xs text-gray-400 mb-4">
            {isVariant
              ? 'Users matching a rule return that rule\'s value (or the default value if no per-rule value is set), bypassing the rollout cap.'
              : 'Users matching any rule always see the flag as enabled, bypassing the rollout cap.'}
          </p>
          <RuleEditor
            rules={flag.rules as TargetingRule[]}
            onChange={rules => setField('rules', rules)}
            flagType={flagType}
            segments={segments}
          />
        </SectionCard>

        <SectionCard title="Tags & ownership">
          <div className="space-y-4">
            <TagsInput tags={flag.tags ?? []} onChange={tags => setField('tags', tags)} />
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1.5">Owner</label>
              <input
                type="email"
                value={flag.owner_email ?? ''}
                onChange={e => setField('owner_email', e.target.value || null)}
                placeholder="owner@example.com"
                className={inputClass}
              />
              <p className="mt-1.5 text-xs text-gray-400">
                Who's responsible for this flag — useful when deciding what's safe to clean up.
              </p>
            </div>
          </div>
        </SectionCard>

        <SectionCard title="Prerequisites">
          <p className="text-xs text-gray-400 mb-4">
            Require other flags to be enabled (or resolve to a specific value) before this flag's
            own rules and rollout are even considered. Checked first — if any prerequisite fails,
            this flag evaluates as disabled.
          </p>
          <PrerequisitesEditor
            prerequisites={flag.prerequisites ?? []}
            candidates={otherFlags}
            onChange={prerequisites => setField('prerequisites', prerequisites)}
          />
        </SectionCard>

        {isEdit && flagKey && (
          <SectionCard title="Scheduled changes">
            <div className="space-y-4">
              {scheduledChanges.filter(c => !c.executed_at).length > 0 && (
                <div className="space-y-2">
                  <p className="text-xs font-medium text-gray-500 uppercase tracking-wide">
                    Pending
                  </p>
                  {scheduledChanges
                    .filter(c => !c.executed_at)
                    .map(sc => (
                      <div
                        key={sc.id}
                        className="flex items-center justify-between gap-3 bg-indigo-50 rounded-lg px-3 py-2 text-sm"
                      >
                        <span className="text-indigo-700">
                          <strong>{sc.patch.is_enabled ? 'Enable' : 'Disable'}</strong>
                          {' at '}
                          {new Date(sc.scheduled_at).toLocaleString()}
                        </span>
                        <button
                          type="button"
                          onClick={() => void cancelScheduledChange(sc.id)}
                          className="text-red-400 hover:text-red-600"
                        >
                          <Trash2 className="w-3.5 h-3.5" />
                        </button>
                      </div>
                    ))}
                </div>
              )}

              {/* Scheduling posts on its own — nesting a <form> inside the flag
                  form is invalid HTML, so this is a plain div and the button
                  calls the handler directly. */}
              <div className="flex items-end gap-3 flex-wrap">
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-1.5">Action</label>
                  <select
                    value={scheduleAction}
                    onChange={e => setScheduleAction(e.target.value as 'enable' | 'disable')}
                    className={selectClass + ' w-auto'}
                  >
                    <option value="enable">Enable flag</option>
                    <option value="disable">Disable flag</option>
                  </select>
                </div>
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-1.5">
                    At (local time)
                  </label>
                  <input
                    type="datetime-local"
                    value={scheduleAt}
                    onChange={e => setScheduleAt(e.target.value)}
                    className={inputClass + ' w-auto'}
                  />
                </div>
                <button
                  type="button"
                  onClick={e => void handleSchedule(e)}
                  disabled={scheduleSaving || !scheduleAt}
                  className="px-4 py-2.5 bg-indigo-600 hover:bg-indigo-700 disabled:opacity-50 text-white text-sm font-semibold rounded-xl transition-all"
                >
                  {scheduleSaving ? 'Scheduling…' : 'Schedule'}
                </button>
              </div>
            </div>
          </SectionCard>
        )}
      </div>

      {/* Pinned action bar — stays reachable however long the form scrolls. */}
      <div className="flex items-center gap-3 border-t border-gray-100 bg-white px-6 py-4">
        <button
          type="submit"
          disabled={saving}
          className="flex items-center gap-2 px-6 py-2.5 bg-brand-600 hover:bg-brand-700 disabled:opacity-60 text-white text-sm font-bold rounded-xl transition-all shadow-lg shadow-brand-200 hover:shadow-brand-300"
        >
          {saving ? (
            <span className="inline-block w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
          ) : (
            <Save className="w-4 h-4" />
          )}
          {saving ? 'Saving…' : isEdit ? 'Save changes' : 'Create flag'}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="px-4 py-2.5 text-sm font-medium text-gray-500 hover:text-gray-800 transition-colors"
        >
          Cancel
        </button>
        {isEdit && flagKey && (
          <button
            type="button"
            onClick={() => void handleArchiveToggle()}
            disabled={archiving}
            className="ml-auto flex items-center gap-2 px-4 py-2.5 text-sm font-medium text-gray-500 hover:text-gray-800 disabled:opacity-50 transition-colors"
          >
            {flag.archived_at ? (
              <><ArchiveRestore className="w-4 h-4" /> {archiving ? 'Unarchiving…' : 'Unarchive'}</>
            ) : (
              <><Archive className="w-4 h-4" /> {archiving ? 'Archiving…' : 'Archive'}</>
            )}
          </button>
        )}
      </div>
    </form>
  )
}
