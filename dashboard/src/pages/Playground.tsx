import { useEffect, useMemo, useState } from 'react'
import { FlaskRound, Play, RotateCcw, AlertTriangle } from 'lucide-react'
import { api } from '../api'
import type { EvaluationReason, Flag, FlagValue, PlaygroundEvaluation, TargetingRule } from '../types'
import { useEnvironment } from '../context/EnvironmentContext'

// ---------------------------------------------------------------------------
// Evaluation playground: "what would an SDK answer for this user?"
//
// The evaluation runs on the server with the SDK engine and the configuration
// revision SDKs are served. Nothing here re-implements targeting: this page
// only collects a context and displays the explanation. Nothing is stored.
// ---------------------------------------------------------------------------

const MAX_ATTRIBUTES = 100

const REASON_TEXT: Record<EvaluationReason, string> = {
  DISABLED: 'The flag is switched off.',
  PREREQUISITE_FAILED: 'A prerequisite flag is not satisfied, so the flag resolves as off.',
  TARGETING_MATCH: 'A targeting rule matched.',
  ROLLOUT_EXCLUDED: 'The user is outside the percentage rollout.',
  SPLIT: 'Hashing the user key decided: inside a partial rollout, or a weighted variant.',
  DEFAULT: 'The flag has rules, none matched: the default value.',
  STATIC: 'No rules, partial rollout or variants: every user gets this value.',
}

type Attributes = Record<string, string | number | boolean>

/** Parses the attributes editor; returns an error message instead of throwing. */
export function parseAttributes(text: string): { attributes?: Attributes; error?: string } {
  if (text.trim() === '') return { attributes: {} }
  let parsed: unknown
  try {
    parsed = JSON.parse(text)
  } catch {
    return { error: 'Attributes must be valid JSON, e.g. {"plan": "pro"}.' }
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return { error: 'Attributes must be a JSON object.' }
  }
  const entries = Object.entries(parsed as Record<string, unknown>)
  if (entries.length > MAX_ATTRIBUTES) return { error: `At most ${MAX_ATTRIBUTES} attributes.` }
  const attributes: Attributes = {}
  for (const [name, value] of entries) {
    if (typeof value !== 'string' && typeof value !== 'number' && typeof value !== 'boolean') {
      return { error: `"${name}" must be a string, number or boolean.` }
    }
    attributes[name] = value
  }
  return { attributes }
}

/** An example context that exercises each of the flag's own rules. */
export function exampleContext(flag: Flag | undefined): Attributes {
  const out: Attributes = {}
  for (const rule of flag?.rules ?? []) {
    if (rule.attribute && rule.values?.length && !(rule.attribute in out)) {
      out[rule.attribute] = rule.values[0]
    }
  }
  return out
}

function describeRule(rule: TargetingRule): string {
  if (rule.segment_key) return `in segment “${rule.segment_key}”`
  return `${rule.attribute} ${rule.operator.replace(/_/g, ' ')} ${rule.values.join(', ')}`
}

function formatValue(value: FlagValue | undefined): string {
  if (value === undefined) return '—'
  return typeof value === 'string' ? JSON.stringify(value) : JSON.stringify(value, null, 2)
}

function valueType(value: FlagValue): string {
  if (value === null) return 'null'
  if (Array.isArray(value)) return 'array'
  return typeof value
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-4 py-2.5 border-b border-gray-50 last:border-none">
      <span className="text-xs font-bold text-gray-400 uppercase tracking-widest shrink-0 pt-0.5">{label}</span>
      <div className="text-sm text-gray-900 text-right min-w-0 break-words">{children}</div>
    </div>
  )
}

export default function Playground() {
  const { activeEnv } = useEnvironment()
  const [flags, setFlags] = useState<Flag[]>([])
  const [flagKey, setFlagKey] = useState('')
  const [targetingKey, setTargetingKey] = useState('user-123')
  const [attributesText, setAttributesText] = useState('{}')
  const [result, setResult] = useState<PlaygroundEvaluation | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    setFlags([])
    setFlagKey('')
    setResult(null)
    if (!activeEnv) return
    let cancelled = false
    api
      .listFlags(activeEnv.id)
      .then(list => {
        if (cancelled) return
        setFlags(list)
        setFlagKey(list[0]?.key ?? '')
      })
      .catch(e => !cancelled && setError(e instanceof Error ? e.message : String(e)))
    return () => {
      cancelled = true
    }
  }, [activeEnv])

  const flag = useMemo(() => flags.find(f => f.key === flagKey), [flags, flagKey])
  const parsed = useMemo(() => parseAttributes(attributesText), [attributesText])

  const reset = () => {
    setTargetingKey('user-123')
    setAttributesText('{}')
    setResult(null)
    setError(null)
  }

  const evaluate = async () => {
    if (!activeEnv || !flagKey || parsed.error || !parsed.attributes) return
    setLoading(true)
    setError(null)
    try {
      setResult(await api.evaluateFlag(activeEnv.id, flagKey, targetingKey, parsed.attributes))
    } catch (e) {
      setResult(null)
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  const inputClass =
    'w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-sm text-gray-900 focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all'

  return (
    <div className="w-full space-y-5">
      <div className="premium-card shadow-premium-lg border-none">
        <div className="flex items-start gap-5 px-6 py-5 border-b border-gray-50 bg-white">
          <div className="w-10 h-10 rounded-xl bg-brand-50 flex items-center justify-center shrink-0">
            <FlaskRound className="w-5 h-5 text-brand-600" />
          </div>
          <div className="flex-1">
            <h1 className="text-lg font-bold text-gray-900">Evaluation Playground</h1>
            <p className="text-sm text-gray-500 mt-0.5">
              See what an SDK would answer for a user in <strong>{activeEnv?.name ?? '…'}</strong>, and why.
              Evaluated on the server with the SDK engine; nothing is changed or stored.
            </p>
          </div>
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-5">
        {/* Left: context */}
        <div className="premium-card p-6 space-y-4">
          <label className="block">
            <span className="text-xs font-bold text-gray-400 uppercase tracking-widest">Flag</span>
            <select
              aria-label="Flag"
              className={`${inputClass} mt-1.5 font-mono`}
              value={flagKey}
              onChange={e => {
                setFlagKey(e.target.value)
                setResult(null)
              }}
            >
              {flags.length === 0 && <option value="">No flags in this environment</option>}
              {flags.map(f => (
                <option key={f.key} value={f.key}>
                  {f.key}
                </option>
              ))}
            </select>
          </label>

          <label className="block">
            <span className="text-xs font-bold text-gray-400 uppercase tracking-widest">Targeting key (user ID)</span>
            <input
              aria-label="Targeting key"
              className={`${inputClass} mt-1.5 font-mono`}
              value={targetingKey}
              maxLength={512}
              onChange={e => setTargetingKey(e.target.value)}
            />
          </label>

          <label className="block">
            <span className="text-xs font-bold text-gray-400 uppercase tracking-widest">Attributes (JSON)</span>
            <textarea
              aria-label="Attributes"
              className={`${inputClass} mt-1.5 font-mono h-40 resize-y`}
              spellCheck={false}
              value={attributesText}
              onChange={e => setAttributesText(e.target.value)}
            />
            {parsed.error && (
              <span role="alert" className="block text-xs text-red-600 mt-1">
                {parsed.error}
              </span>
            )}
          </label>

          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="px-3 py-1.5 text-xs font-semibold rounded-lg bg-gray-50 hover:bg-gray-100 text-gray-700"
              onClick={() => setAttributesText(JSON.stringify(exampleContext(flag), null, 2))}
              disabled={!flag}
            >
              Example from this flag’s rules
            </button>
            <button
              type="button"
              className="px-3 py-1.5 text-xs font-semibold rounded-lg bg-gray-50 hover:bg-gray-100 text-gray-700"
              onClick={() => setAttributesText('{}')}
            >
              No attributes
            </button>
          </div>

          <div className="flex gap-2 pt-2">
            <button
              type="button"
              onClick={evaluate}
              disabled={loading || !flagKey || !!parsed.error}
              className="flex items-center gap-1.5 px-4 py-2 bg-brand-600 hover:bg-brand-700 disabled:opacity-50 text-white text-sm font-bold rounded-lg transition-all"
            >
              <Play className="w-4 h-4" /> {loading ? 'Evaluating…' : 'Evaluate'}
            </button>
            <button
              type="button"
              onClick={reset}
              className="flex items-center gap-1.5 px-4 py-2 bg-white border border-gray-100 hover:bg-gray-50 text-gray-700 text-sm font-semibold rounded-lg"
            >
              <RotateCcw className="w-4 h-4" /> Reset
            </button>
          </div>
        </div>

        {/* Right: result */}
        <div className="premium-card p-6" aria-live="polite">
          {error && (
            <div role="alert" className="flex items-start gap-2 text-sm text-red-700 bg-red-50 rounded-xl px-4 py-3 mb-4">
              <AlertTriangle className="w-4 h-4 shrink-0 mt-0.5" /> <span>{error}</span>
            </div>
          )}
          {!result && !error && (
            <p className="text-sm text-gray-400">Choose a flag, enter a user, and press Evaluate.</p>
          )}
          {result && (
            <div>
              <div className="flex items-center gap-3 mb-4">
                <span
                  data-testid="result-state"
                  className={`px-2.5 py-1 rounded-lg text-xs font-bold ${
                    result.enabled ? 'bg-emerald-50 text-emerald-700' : 'bg-red-50 text-red-700'
                  }`}
                >
                  {result.enabled ? 'ENABLED' : 'DISABLED'}
                </span>
                <span className="font-mono text-sm text-gray-900">{result.flag_key}</span>
              </div>
              <Row label="Value">
                <pre data-testid="result-value" className="font-mono text-sm whitespace-pre-wrap text-right">
                  {formatValue(result.value)}
                </pre>
              </Row>
              <Row label="Type">{valueType(result.value)} <span className="text-gray-400">({result.flag_type})</span></Row>
              <Row label="Reason">
                <span data-testid="result-reason" className="font-mono font-semibold">{result.reason}</span>
                <p className="text-xs text-gray-500 mt-0.5">{REASON_TEXT[result.reason]}</p>
              </Row>
              {result.matched_rule && (
                <Row label="Matched rule">
                  <span className="font-mono">
                    #{result.matched_rule.source_index + 1}: {describeRule(result.matched_rule.rule)}
                  </span>
                  {result.matched_rule.segment_key && (
                    <p className="text-xs text-gray-500 mt-0.5">
                      via segment “{result.matched_rule.segment_key}”, its rule #
                      {(result.matched_rule.segment_rule_index ?? 0) + 1}
                    </p>
                  )}
                </Row>
              )}
              {result.rollout && (
                <Row label="Rollout">
                  bucket <span className="font-mono">{result.rollout.bucket}</span> of 0–99; inside when below{' '}
                  <span className="font-mono">{result.rollout.percentage}</span>%
                </Row>
              )}
              {result.variant && (
                <Row label="Variant">
                  #{result.variant.index + 1} (weight {result.variant.weight} of {result.variant.total_weight})
                </Row>
              )}
              {result.prerequisites.length > 0 && (
                <Row label="Prerequisites">
                  <ul className="space-y-1">
                    {result.prerequisites.map(p => (
                      <li key={p.flag_key} className="font-mono text-xs">
                        {p.flag_key}: <strong>{p.outcome.replace(/_/g, ' ')}</strong>
                        {p.result && ` → ${formatValue(p.result.value)}`}
                        {p.revision !== undefined && <span className="text-gray-400"> (rev {p.revision})</span>}
                      </li>
                    ))}
                  </ul>
                </Row>
              )}
              {result.missing_attributes.length > 0 && (
                <Row label="Not provided">
                  <span className="font-mono text-xs text-amber-700">{result.missing_attributes.join(', ')}</span>
                  <p className="text-xs text-gray-500 mt-0.5">Attributes the rules read that this context lacks.</p>
                </Row>
              )}
              <Row label="Revision">
                <span className="font-mono">{result.revision}</span>
              </Row>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
