// Pure functions over flag JSON as the Checkgate API returns it: readable
// explanations, cross-environment diffs, dependency graphs and audit
// summaries. Nothing here performs I/O.
//
// Every string these return that contains flag data (descriptions, rule
// attributes and values, variant values) is user-controlled. Callers return
// it inside the delimited data block, never as instructions.

const OPERATOR_TEXT = Object.freeze({
  equals: 'equals one of',
  not_equals: 'does not equal any of',
  contains: 'contains one of',
  starts_with: 'starts with one of',
  ends_with: 'ends with one of',
  greater_than: 'is a number greater than one of',
  greater_than_or_equal: 'is a number greater than or equal to one of',
  less_than: 'is a number less than one of',
  less_than_or_equal: 'is a number less than or equal to one of',
})

/** Evaluation-relevant fields (what SDKs act on). */
export const EVALUATION_FIELDS = Object.freeze([
  'is_enabled',
  'rollout_percentage',
  'flag_type',
  'rules',
  'default_value',
  'disabled_value',
  'variants',
  'prerequisites',
])

/** Management metadata — never affects evaluation. */
export const METADATA_FIELDS = Object.freeze(['description', 'tags', 'owner_email', 'archived_at'])

function show(value) {
  return JSON.stringify(value === undefined ? null : value)
}

function enabledDefault(flag) {
  return (flag.flag_type ?? 'boolean') === 'boolean' ? true : null
}

function servedWhenOn(flag, ruleVariant) {
  if (ruleVariant !== undefined && ruleVariant !== null) return ruleVariant
  if (flag.default_value !== undefined && flag.default_value !== null) return flag.default_value
  return enabledDefault(flag)
}

function servedWhenOff(flag) {
  return flag.disabled_value !== undefined && flag.disabled_value !== null ? flag.disabled_value : false
}

function describeRule(rule, segments) {
  if (rule.segment_key) {
    const seg = segments?.[rule.segment_key]
    if (!seg) return `the user is in segment ${show(rule.segment_key)} (segment definition not available)`
    const inner = (seg.rules ?? []).map((r) => describeCondition(r))
    if (inner.length === 0) return `the user is in segment ${show(rule.segment_key)}, which has no rules (matches nobody)`
    return `the user is in segment ${show(rule.segment_key)}, i.e. ${inner.join(' OR ')}`
  }
  return describeCondition(rule)
}

function describeCondition(rule) {
  const op = OPERATOR_TEXT[rule.operator ?? 'equals'] ?? `${show(rule.operator)} (unknown operator)`
  const values = (rule.values ?? []).map(show).join(', ')
  let text = `attribute ${show(rule.attribute ?? '')} ${op} [${values}]`
  if (rule.operator === 'not_equals') text += ' (also matches users WITHOUT this attribute)'
  return text
}

/**
 * Explain, step by step and in evaluation order, what a flag serves.
 * Mirrors `checkgate_core::evaluator::evaluate_variant`.
 *
 * @param {object} flag  flag JSON (as from GET /flags/{key})
 * @param {Record<string, object>} [segments]  segment definitions by key, to expand segment rules
 * @returns {{ steps: string[], summary: string }}
 */
export function explainFlag(flag, segments = {}) {
  const steps = []
  const off = show(servedWhenOff(flag))
  if (!flag.is_enabled) {
    steps.push(`1. The flag is DISABLED: every user gets the off value ${off}. Nothing below is evaluated.`)
    describeRest(flag, segments, steps, true)
    return { steps, summary: `Disabled — serves ${off} to everyone.` }
  }
  steps.push('1. The flag is ENABLED.')
  describeRest(flag, segments, steps, false)
  return { steps, summary: 'Enabled — see the steps for who gets what.' }
}

function describeRest(flag, segments, steps, inactive) {
  const prefix = inactive ? '(inactive while disabled) ' : ''
  const off = show(servedWhenOff(flag))
  let n = steps.length + 1
  const prereqs = flag.prerequisites ?? []
  if (prereqs.length > 0) {
    const list = prereqs
      .map((p) => (p.required_value !== undefined && p.required_value !== null
        ? `${show(p.flag_key)} must be enabled for the user and serve ${show(p.required_value)}`
        : `${show(p.flag_key)} must be enabled for the user`))
      .join('; ')
    steps.push(`${n++}. ${prefix}Prerequisites (checked first, all must hold, a missing prerequisite flag fails): ${list}. Otherwise the user gets the off value ${off}.`)
  }
  const rules = flag.rules ?? []
  rules.forEach((rule, i) => {
    steps.push(`${n++}. ${prefix}Rule ${i + 1} (first match wins): if ${describeRule(rule, segments)} → serve ${show(servedWhenOn(flag, rule.variant))}. Rules bypass the rollout percentage.`)
  })
  const pct = flag.rollout_percentage
  if (pct === 0) {
    steps.push(`${n++}. ${prefix}Rollout 0 %: users who matched no rule get the off value ${off}.`)
    return
  }
  if (typeof pct === 'number' && pct < 100) {
    steps.push(`${n++}. ${prefix}Rollout ${pct} %: of the users who matched no rule, a deterministic ${pct} % (hash of flag key + user key) continue; the rest get the off value ${off}.`)
  }
  const variants = flag.variants ?? []
  if (variants.length > 0) {
    const total = variants.reduce((a, v) => a + (Number(v.weight) || 0), 0)
    const split = variants
      .map((v) => `${show(v.value)} ${total > 0 ? `${((100 * (Number(v.weight) || 0)) / total).toFixed(1)} %` : '(weight 0)'}`)
      .join(', ')
    steps.push(`${n++}. ${prefix}Everyone else is split across variants: ${split}${total === 0 ? ' — all weights are 0, so everyone gets the first variant' : ''}.`)
  } else {
    steps.push(`${n++}. ${prefix}Everyone else gets ${show(servedWhenOn(flag))}.`)
  }
}

function equalJson(a, b) {
  return JSON.stringify(a ?? null) === JSON.stringify(b ?? null)
}

/**
 * Compare one flag between two environments.
 * @param {object|null} a  flag in environment A, or null if missing
 * @param {object|null} b  flag in environment B, or null if missing
 */
export function diffFlag(a, b) {
  if (!a || !b) {
    return {
      identical: false,
      present_in_a: Boolean(a),
      present_in_b: Boolean(b),
      evaluation_differences: [],
      metadata_differences: [],
    }
  }
  const pick = (fields) => fields
    .filter((f) => !equalJson(a[f], b[f]))
    .map((field) => ({ field, a: a[field] ?? null, b: b[field] ?? null }))
  const evaluation = pick(EVALUATION_FIELDS)
  const metadata = pick(METADATA_FIELDS)
  return {
    identical: evaluation.length === 0 && metadata.length === 0,
    evaluates_identically: evaluation.length === 0,
    present_in_a: true,
    present_in_b: true,
    evaluation_differences: evaluation,
    metadata_differences: metadata,
  }
}

/**
 * Prerequisites of `key` and the flags that depend on it, within one
 * environment's flag list.
 */
export function dependencies(key, flags) {
  const byKey = new Map(flags.map((f) => [f.key, f]))
  const target = byKey.get(key)
  const prerequisites = (target?.prerequisites ?? []).map((p) => {
    const f = byKey.get(p.flag_key)
    return {
      flag_key: p.flag_key,
      required_value: p.required_value ?? null,
      exists: Boolean(f),
      is_enabled: f ? Boolean(f.is_enabled) : null,
      note: f ? undefined : 'missing — the prerequisite fails closed, so the dependent flag serves its off value',
    }
  })
  const dependents = flags
    .filter((f) => (f.prerequisites ?? []).some((p) => p.flag_key === key))
    .map((f) => ({
      flag_key: f.key,
      is_enabled: Boolean(f.is_enabled),
      required_value: (f.prerequisites.find((p) => p.flag_key === key)?.required_value) ?? null,
    }))
  return { found: Boolean(target), prerequisites, dependents }
}

/** Audit entry with only the fields that changed, instead of two full blobs. */
export function summarizeAudit(entry) {
  const before = entry.before_data ?? null
  const after = entry.after_data ?? null
  const keys = new Set([...Object.keys(before ?? {}), ...Object.keys(after ?? {})])
  const changes = []
  for (const k of keys) {
    if (!equalJson(before?.[k], after?.[k])) changes.push({ field: k, before: before?.[k] ?? null, after: after?.[k] ?? null })
  }
  return {
    id: entry.id,
    created_at: entry.created_at,
    flag_key: entry.flag_key,
    action: entry.action,
    actor_email: entry.actor_email ?? null,
    metadata: entry.metadata ?? null,
    changes,
  }
}

/** Connection counts per environment. Drops addresses, ids and key names. */
export function summarizeConnections(list) {
  const counts = new Map()
  let unattributed = 0
  for (const c of list) {
    const env = typeof c?.environment_id === 'string' ? c.environment_id : null
    if (env === null) unattributed += 1
    else counts.set(env, (counts.get(env) ?? 0) + 1)
  }
  return {
    total: list.length,
    per_environment: [...counts.entries()]
      .sort((x, y) => y[1] - x[1] || x[0].localeCompare(y[0]))
      .map(([environment_id, connections]) => ({ environment_id, connections })),
    without_environment: unattributed,
    note: 'Live SSE connections held by the server instance that answered; behind a load balancer each instance reports only its own.',
  }
}
