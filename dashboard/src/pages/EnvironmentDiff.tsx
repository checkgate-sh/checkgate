import { useCallback, useEffect, useState } from 'react'
import { ArrowLeftRight, ArrowRight, GitCompare, RotateCcw } from 'lucide-react'
import { api } from '../api'
import type { Flag } from '../types'
import { useEnvironment, type Environment } from '../context/EnvironmentContext'

// ---------------------------------------------------------------------------
// Diffing — compares only the fields that affect evaluation. Tags, owner,
// and archival state are dashboard-only metadata (see FlagForm.tsx) and
// deliberately excluded so they don't show up as noisy "differences" between
// otherwise-identical flags.
// ---------------------------------------------------------------------------

const EVAL_FIELDS = [
  'is_enabled',
  'rollout_percentage',
  'rules',
  'flag_type',
  'default_value',
  'disabled_value',
  'variants',
  'prerequisites',
] as const

function diffFields(a: Flag, b: Flag): string[] {
  const changed: string[] = []
  for (const k of EVAL_FIELDS) {
    if (JSON.stringify(a[k]) !== JSON.stringify(b[k])) changed.push(k)
  }
  return changed
}

interface DiffResult {
  onlyInA: Flag[]
  onlyInB: Flag[]
  different: { a: Flag; b: Flag; fields: string[] }[]
  identicalCount: number
}

function computeDiff(flagsA: Flag[], flagsB: Flag[]): DiffResult {
  const mapB = new Map(flagsB.map(f => [f.key, f]))
  const seenInA = new Set(flagsA.map(f => f.key))
  const onlyInA: Flag[] = []
  const different: DiffResult['different'] = []
  let identicalCount = 0

  for (const a of flagsA) {
    const b = mapB.get(a.key)
    if (!b) {
      onlyInA.push(a)
      continue
    }
    const fields = diffFields(a, b)
    if (fields.length > 0) different.push({ a, b, fields })
    else identicalCount++
  }
  const onlyInB = flagsB.filter(b => !seenInA.has(b.key))

  return { onlyInA, onlyInB, different, identicalCount }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

function EnvSelect({
  environments,
  value,
  onChange,
  exclude,
}: {
  environments: Environment[]
  value: string
  onChange: (id: string) => void
  exclude?: string
}) {
  return (
    <select
      value={value}
      onChange={e => onChange(e.target.value)}
      className="bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-sm text-gray-900 font-semibold focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 shadow-premium transition-all"
    >
      {environments.filter(e => e.id !== exclude).map(e => (
        <option key={e.id} value={e.id}>{e.name}</option>
      ))}
    </select>
  )
}

function FlagRow({
  flagKey,
  fields,
  action,
}: {
  flagKey: string
  fields?: string[]
  action?: { label: string; onClick: () => void; pending: boolean }
}) {
  return (
    <div className="flex items-center justify-between gap-3 px-4 py-3 bg-white border border-gray-100 rounded-xl">
      <div className="min-w-0">
        <span className="font-mono text-sm text-gray-900 font-medium">{flagKey}</span>
        {fields && fields.length > 0 && (
          <div className="flex flex-wrap gap-1 mt-1.5">
            {fields.map(f => (
              <span key={f} className="px-1.5 py-0.5 bg-amber-50 text-amber-700 rounded text-[9px] font-mono font-medium">
                {f}
              </span>
            ))}
          </div>
        )}
      </div>
      {action && (
        <button
          onClick={action.onClick}
          disabled={action.pending}
          className="shrink-0 flex items-center gap-1.5 px-3 py-1.5 bg-brand-600 hover:bg-brand-700 disabled:opacity-50 text-white text-xs font-bold rounded-lg transition-all"
        >
          {action.pending ? 'Syncing…' : action.label}
          {!action.pending && <ArrowRight className="w-3 h-3" />}
        </button>
      )}
    </div>
  )
}

function Section({ title, count, children }: { title: string; count: number; children: React.ReactNode }) {
  if (count === 0) return null
  return (
    <div>
      <p className="text-xs font-bold text-gray-400 uppercase tracking-widest mb-2">{title} ({count})</p>
      <div className="space-y-2">{children}</div>
    </div>
  )
}

export default function EnvironmentDiff() {
  const { environments, activeEnv } = useEnvironment()
  const [envAId, setEnvAId] = useState('')
  const [envBId, setEnvBId] = useState('')
  const [flagsA, setFlagsA] = useState<Flag[]>([])
  const [flagsB, setFlagsB] = useState<Flag[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [syncingKey, setSyncingKey] = useState<string | null>(null)

  // Default to the active environment vs. the first other environment.
  useEffect(() => {
    if (environments.length < 2) return
    if (!envAId) setEnvAId(activeEnv?.id ?? environments[0].id)
    if (!envBId) {
      const other = environments.find(e => e.id !== (activeEnv?.id ?? environments[0].id))
      if (other) setEnvBId(other.id)
    }
  }, [environments, activeEnv, envAId, envBId])

  const load = useCallback(async () => {
    if (!envAId || !envBId || envAId === envBId) return
    setLoading(true)
    setError(null)
    try {
      const [a, b] = await Promise.all([
        api.listFlags(envAId, { includeArchived: true }),
        api.listFlags(envBId, { includeArchived: true }),
      ])
      setFlagsA(a)
      setFlagsB(b)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load flags')
    } finally {
      setLoading(false)
    }
  }, [envAId, envBId])

  useEffect(() => { void load() }, [load])

  async function sync(key: string, fromEnvId: string, toEnvId: string) {
    setSyncingKey(key)
    try {
      await api.promoteFlag(fromEnvId, key, toEnvId)
      await load()
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Sync failed')
    } finally {
      setSyncingKey(null)
    }
  }

  const envA = environments.find(e => e.id === envAId)
  const envB = environments.find(e => e.id === envBId)
  const diff = envAId && envBId && envAId !== envBId ? computeDiff(flagsA, flagsB) : null

  return (
    <div className="w-full space-y-5">
      <div className="premium-card shadow-premium-lg border-none">
        <div className="flex items-start gap-5 px-6 py-5 border-b border-gray-50 bg-white">
          <div className="w-10 h-10 rounded-xl bg-brand-50 flex items-center justify-center shrink-0">
            <GitCompare className="w-5 h-5 text-brand-600" />
          </div>
          <div className="flex-1">
            <h2 className="text-gray-900 font-display font-bold text-sm tracking-tight">Compare environments</h2>
            <p className="text-gray-400 text-xs mt-0.5">
              See what's different before promoting — tags, owner, and archival state aren't
              compared since they don't affect evaluation.
            </p>
          </div>
        </div>

        <div className="p-6 space-y-5">
          <div className="flex items-center gap-3 flex-wrap">
            <EnvSelect environments={environments} value={envAId} onChange={setEnvAId} />
            <button
              onClick={() => { setEnvAId(envBId); setEnvBId(envAId) }}
              className="p-2 rounded-lg text-gray-400 hover:text-gray-700 hover:bg-gray-100 transition-colors"
              aria-label="Swap environments"
              title="Swap"
            >
              <ArrowLeftRight className="w-4 h-4" />
            </button>
            <EnvSelect environments={environments} value={envBId} onChange={setEnvBId} exclude={envAId} />
            <button
              onClick={() => void load()}
              className="ml-auto flex items-center gap-1.5 text-sm text-gray-500 hover:text-gray-800 transition-colors"
            >
              <RotateCcw className="w-3.5 h-3.5" /> Refresh
            </button>
          </div>

          {error && (
            <div className="p-3.5 rounded-lg bg-red-50 border border-red-200 text-red-600 text-sm">{error}</div>
          )}

          {envAId === envBId ? (
            <p className="text-center text-gray-400 text-sm py-6">Pick two different environments to compare.</p>
          ) : loading ? (
            <div className="flex items-center justify-center h-32 text-gray-400 text-sm">Loading…</div>
          ) : diff && envA && envB ? (
            diff.onlyInA.length === 0 && diff.onlyInB.length === 0 && diff.different.length === 0 ? (
              <p className="text-center text-brand-600 text-sm py-6 font-medium">
                {envA.name} and {envB.name} are in sync ({diff.identicalCount} flag{diff.identicalCount !== 1 ? 's' : ''}).
              </p>
            ) : (
              <div className="space-y-6">
                <Section title={`Only in ${envA.name}`} count={diff.onlyInA.length}>
                  {diff.onlyInA.map(f => (
                    <FlagRow
                      key={f.key}
                      flagKey={f.key}
                      action={{
                        label: `Copy to ${envB.name}`,
                        onClick: () => void sync(f.key, envA.id, envB.id),
                        pending: syncingKey === f.key,
                      }}
                    />
                  ))}
                </Section>

                <Section title={`Only in ${envB.name}`} count={diff.onlyInB.length}>
                  {diff.onlyInB.map(f => (
                    <FlagRow
                      key={f.key}
                      flagKey={f.key}
                      action={{
                        label: `Copy to ${envA.name}`,
                        onClick: () => void sync(f.key, envB.id, envA.id),
                        pending: syncingKey === f.key,
                      }}
                    />
                  ))}
                </Section>

                <Section title="Different" count={diff.different.length}>
                  {diff.different.map(({ a, fields }) => (
                    <FlagRow
                      key={a.key}
                      flagKey={a.key}
                      fields={fields}
                      action={{
                        label: `${envA.name} → ${envB.name}`,
                        onClick: () => void sync(a.key, envA.id, envB.id),
                        pending: syncingKey === a.key,
                      }}
                    />
                  ))}
                </Section>

                {diff.identicalCount > 0 && (
                  <p className="text-xs text-gray-400">
                    {diff.identicalCount} other flag{diff.identicalCount !== 1 ? 's' : ''} identical.
                  </p>
                )}
              </div>
            )
          ) : (
            <p className="text-center text-gray-400 text-sm py-6">Need at least two environments to compare.</p>
          )}
        </div>
      </div>
    </div>
  )
}
