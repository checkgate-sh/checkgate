import { Trash2, Plus, Tags } from 'lucide-react'
import type { FlagType, FlagValue, Segment, TargetingRule, Operator } from '../types'

const OPERATORS: { value: Operator; label: string }[] = [
  { value: 'equals',                label: 'equals' },
  { value: 'not_equals',            label: 'does not equal' },
  { value: 'contains',              label: 'contains' },
  { value: 'starts_with',           label: 'starts with' },
  { value: 'ends_with',             label: 'ends with' },
  { value: 'greater_than',          label: '> (greater than)' },
  { value: 'greater_than_or_equal', label: '≥ (greater than or equal)' },
  { value: 'less_than',             label: '< (less than)' },
  { value: 'less_than_or_equal',    label: '≤ (less than or equal)' },
]

const inputClass =
  'w-full bg-white border border-gray-100 rounded-xl px-4 py-2 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium'

const selectClass =
  'w-full bg-white border border-gray-100 rounded-xl px-4 py-2 text-gray-900 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium appearance-none'

interface RuleEditorProps {
  rules: TargetingRule[]
  onChange: (rules: TargetingRule[]) => void
  flagType?: FlagType
  /** Available segments for this environment — enables the "Use segment" option. */
  segments?: Segment[]
}

function parseVariant(raw: string, flagType: FlagType): FlagValue {
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

function variantToString(v: FlagValue): string {
  if (v == null) return ''
  if (typeof v === 'object') return JSON.stringify(v)
  return String(v)
}

export default function RuleEditor({
  rules,
  onChange,
  flagType = 'boolean',
  segments = [],
}: RuleEditorProps) {
  const isVariant = flagType !== 'boolean'

  function addRule() {
    onChange([...rules, { attribute: '', operator: 'equals', values: [''] }])
  }

  function addSegmentRule(segKey: string) {
    onChange([...rules, { attribute: '', operator: 'equals', values: [], segment_key: segKey }])
  }

  function removeRule(i: number) {
    onChange(rules.filter((_, idx) => idx !== i))
  }

  function updateRule(i: number, partial: Partial<TargetingRule>) {
    onChange(rules.map((r, idx) => (idx === i ? { ...r, ...partial } : r)))
  }

  function updateValues(i: number, raw: string) {
    const values = raw.split(',').map(v => v.trim()).filter(Boolean)
    updateRule(i, { values: values.length ? values : [''] })
  }

  function updateVariant(i: number, raw: string) {
    const variant = raw.trim() === '' ? undefined : parseVariant(raw, flagType)
    updateRule(i, { variant })
  }

  return (
    <div className="space-y-3">
      {rules.map((rule, i) => {
        const isSegmentRule = !!rule.segment_key
        return (
          <div
            key={i}
            className={`flex flex-wrap gap-3 items-end p-4 rounded-lg border ${
              isSegmentRule
                ? 'bg-violet-50/50 border-violet-100'
                : 'bg-gray-50 border-gray-200'
            }`}
          >
            {isSegmentRule ? (
              // Segment rule display
              <div className="flex-1 flex items-center gap-2 min-w-0">
                <Tags className="w-4 h-4 text-violet-500 shrink-0" />
                <div className="flex-1 min-w-0">
                  <label className="block text-xs font-medium text-violet-600 mb-1">Segment</label>
                  <select
                    value={rule.segment_key}
                    onChange={e =>
                      updateRule(i, {
                        segment_key: e.target.value || undefined,
                        attribute: '',
                        values: [],
                      })
                    }
                    className={`${selectClass} border-violet-100 focus:ring-violet-500/10 focus:border-violet-500/30`}
                  >
                    {segments.map(s => (
                      <option key={s.key} value={s.key}>
                        {s.name} ({s.key})
                      </option>
                    ))}
                    {segments.length === 0 && (
                      <option value={rule.segment_key}>{rule.segment_key}</option>
                    )}
                  </select>
                </div>

                {isVariant && (
                  <div className="flex-1 min-w-32">
                    <label className="block text-xs font-medium text-gray-500 mb-1.5">
                      Return value <span className="text-gray-400">(optional)</span>
                    </label>
                    <input
                      type={flagType === 'integer' ? 'number' : 'text'}
                      value={variantToString(rule.variant ?? null)}
                      onChange={e => updateVariant(i, e.target.value)}
                      placeholder={flagType === 'integer' ? '0' : 'value'}
                      className={inputClass}
                    />
                  </div>
                )}
              </div>
            ) : (
              // Concrete rule
              <>
                <div className="flex-1 min-w-32">
                  <label className="block text-xs font-medium text-gray-500 mb-1.5">Attribute</label>
                  <input
                    type="text"
                    value={rule.attribute}
                    onChange={e => updateRule(i, { attribute: e.target.value })}
                    placeholder="e.g. email"
                    className={inputClass}
                  />
                </div>

                <div className="w-44">
                  <label className="block text-xs font-medium text-gray-500 mb-1.5">Operator</label>
                  <select
                    value={rule.operator}
                    onChange={e => updateRule(i, { operator: e.target.value as Operator })}
                    className={selectClass}
                  >
                    {OPERATORS.map(op => (
                      <option key={op.value} value={op.value}>{op.label}</option>
                    ))}
                  </select>
                </div>

                <div className="flex-1 min-w-40">
                  <label className="block text-xs font-medium text-gray-500 mb-1.5">
                    Values <span className="text-gray-400">(comma-separated)</span>
                  </label>
                  <input
                    type="text"
                    value={rule.values.join(', ')}
                    onChange={e => updateValues(i, e.target.value)}
                    placeholder="e.g. @acme.com, @example.com"
                    className={inputClass}
                  />
                </div>

                {isVariant && (
                  <div className="flex-1 min-w-32">
                    <label className="block text-xs font-medium text-gray-500 mb-1.5">
                      Return value <span className="text-gray-400">(optional)</span>
                    </label>
                    {flagType === 'json' ? (
                      <textarea
                        rows={1}
                        value={variantToString(rule.variant ?? null)}
                        onChange={e => updateVariant(i, e.target.value)}
                        placeholder='{"key":"val"}'
                        className={`${inputClass} resize-none font-mono text-xs`}
                      />
                    ) : (
                      <input
                        type={flagType === 'integer' ? 'number' : 'text'}
                        value={variantToString(rule.variant ?? null)}
                        onChange={e => updateVariant(i, e.target.value)}
                        placeholder={flagType === 'integer' ? '0' : 'value'}
                        className={`${inputClass} ${flagType === 'string' ? '' : 'font-mono'}`}
                      />
                    )}
                  </div>
                )}
              </>
            )}

            <button
              type="button"
              onClick={() => removeRule(i)}
              className="mb-0.5 p-2 rounded-lg text-gray-400 hover:text-red-500 hover:bg-red-50 transition-colors"
              aria-label="Remove rule"
            >
              <Trash2 className="w-4 h-4" />
            </button>
          </div>
        )
      })}

      <div className="flex items-center gap-3">
        <button
          type="button"
          onClick={addRule}
          className="flex items-center gap-1.5 text-sm text-brand-600 hover:text-brand-700 font-bold transition-colors"
        >
          <Plus className="w-4 h-4" /> Add rule
        </button>

        {segments.length > 0 && (
          <div className="relative group">
            <button
              type="button"
              className="flex items-center gap-1.5 text-sm text-violet-600 hover:text-violet-700 font-bold transition-colors"
            >
              <Tags className="w-4 h-4" /> Add segment
            </button>
            {/* Dropdown */}
            <div className="absolute left-0 top-full mt-1 z-20 hidden group-focus-within:block group-hover:block bg-white border border-gray-100 rounded-xl shadow-lg overflow-hidden min-w-48">
              {segments.map(s => (
                <button
                  key={s.key}
                  type="button"
                  onClick={() => addSegmentRule(s.key)}
                  className="w-full text-left px-4 py-2.5 text-sm hover:bg-violet-50 transition-colors"
                >
                  <span className="font-semibold text-gray-900">{s.name}</span>
                  <span className="block text-[10px] font-mono text-gray-400">{s.key}</span>
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
