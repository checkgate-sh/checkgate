import { useCallback, useEffect, useState } from 'react'
import { Plus, Pencil, Trash2, Tags, X, Check } from 'lucide-react'
import { segmentsApi } from '../api'
import type { Segment, TargetingRule } from '../types'
import { useEnvironment } from '../context/EnvironmentContext'
import RuleEditor from '../components/RuleEditor'

// ---------------------------------------------------------------------------
// Segment form modal (create & edit)
// ---------------------------------------------------------------------------

interface SegmentFormProps {
  envId: string
  initial?: Segment
  onSave: (seg: Segment) => void
  onClose: () => void
}

function SegmentForm({ envId, initial, onSave, onClose }: SegmentFormProps) {
  const [name, setName]         = useState(initial?.name ?? '')
  const [key, setKey]           = useState(initial?.key ?? '')
  const [description, setDesc]  = useState(initial?.description ?? '')
  const [rules, setRules]       = useState<TargetingRule[]>(initial?.rules ?? [])
  const [saving, setSaving]     = useState(false)
  const [error, setError]       = useState('')
  const isEdit = !!initial

  function deriveKey(v: string) {
    return v.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-_]/g, '')
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    setSaving(true)
    setError('')
    try {
      let seg: Segment
      if (isEdit) {
        seg = await segmentsApi.patch(envId, initial!.key, { name, description, rules })
      } else {
        seg = await segmentsApi.create(envId, { name, key, description, rules })
      }
      onSave(seg)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Save failed')
    } finally {
      setSaving(false)
    }
  }

  const inputClass =
    'w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-2 focus:ring-brand-500/20 focus:border-brand-500/40 transition-all shadow-sm'

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm p-4">
      <div className="bg-white rounded-2xl shadow-2xl border border-gray-100 w-full max-w-2xl max-h-[90vh] flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-gray-100">
          <h2 className="font-bold text-gray-900 text-base">
            {isEdit ? `Edit segment: ${initial!.name}` : 'New segment'}
          </h2>
          <button
            onClick={onClose}
            className="p-1.5 rounded-lg text-gray-400 hover:text-gray-700 hover:bg-gray-100 transition-colors"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Body */}
        <form onSubmit={e => void handleSubmit(e)} className="flex-1 overflow-y-auto">
          <div className="px-6 py-5 space-y-4">
            {error && (
              <p className="text-sm text-red-500 bg-red-50 px-4 py-2 rounded-lg">{error}</p>
            )}

            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-xs font-semibold text-gray-600 mb-1.5">Name</label>
                <input
                  required
                  value={name}
                  onChange={e => {
                    setName(e.target.value)
                    if (!isEdit) setKey(deriveKey(e.target.value))
                  }}
                  placeholder="Beta users"
                  className={inputClass}
                />
              </div>

              <div>
                <label className="block text-xs font-semibold text-gray-600 mb-1.5">Key</label>
                <input
                  required
                  readOnly={isEdit}
                  value={key}
                  onChange={e => setKey(e.target.value)}
                  placeholder="beta-users"
                  className={`${inputClass} font-mono ${isEdit ? 'bg-gray-50 text-gray-400 cursor-default' : ''}`}
                />
              </div>
            </div>

            <div>
              <label className="block text-xs font-semibold text-gray-600 mb-1.5">
                Description <span className="font-normal text-gray-400">(optional)</span>
              </label>
              <input
                value={description}
                onChange={e => setDesc(e.target.value)}
                placeholder="Users enrolled in the beta program"
                className={inputClass}
              />
            </div>

            <div>
              <label className="block text-xs font-semibold text-gray-600 mb-2">
                Targeting rules
              </label>
              <RuleEditor rules={rules} onChange={setRules} />
            </div>
          </div>

          {/* Footer */}
          <div className="px-6 py-4 border-t border-gray-100 flex justify-end gap-2 bg-gray-50/50">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 text-sm text-gray-600 font-semibold hover:bg-gray-100 rounded-xl transition-colors"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={saving}
              className="flex items-center gap-2 px-5 py-2 bg-brand-600 hover:bg-brand-700 disabled:opacity-40 disabled:cursor-not-allowed text-white text-sm font-bold rounded-xl transition-all shadow-sm"
            >
              {saving ? (
                'Saving…'
              ) : (
                <>
                  <Check className="w-3.5 h-3.5" />
                  {isEdit ? 'Save changes' : 'Create segment'}
                </>
              )}
            </button>
          </div>
        </form>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

export default function Segments() {
  const { activeEnv } = useEnvironment()
  const [segments, setSegments] = useState<Segment[]>([])
  const [loading, setLoading]   = useState(true)
  const [error, setError]       = useState<string | null>(null)
  const [editing, setEditing]   = useState<Segment | null | 'new'>(null)

  const load = useCallback(async () => {
    if (!activeEnv) return
    setLoading(true)
    setError(null)
    try {
      setSegments(await segmentsApi.list(activeEnv.id))
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load segments')
    } finally {
      setLoading(false)
    }
  }, [activeEnv])

  useEffect(() => {
    void load()
  }, [load])

  async function handleDelete(key: string) {
    if (!activeEnv) return
    if (
      !confirm(
        `Delete segment "${key}"?\n\nFlags that reference this segment will have the matching rules removed on their next evaluation.`,
      )
    )
      return
    try {
      await segmentsApi.delete(activeEnv.id, key)
      setSegments(prev => prev.filter(s => s.key !== key))
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Delete failed')
    }
  }

  function handleSaved(seg: Segment) {
    setSegments(prev => {
      const idx = prev.findIndex(s => s.key === seg.key)
      return idx >= 0 ? prev.map((s, i) => (i === idx ? seg : s)) : [...prev, seg]
    })
    setEditing(null)
  }

  return (
    <div className="w-full space-y-4">
      {/* Modal */}
      {editing !== null && activeEnv && (
        <SegmentForm
          envId={activeEnv.id}
          initial={editing === 'new' ? undefined : editing}
          onSave={handleSaved}
          onClose={() => setEditing(null)}
        />
      )}

      {/* Toolbar */}
      <div className="flex items-center gap-3">
        {activeEnv && (
          <div className="flex items-center gap-1.5 px-3 py-1.5 bg-white border border-gray-100 rounded-xl text-xs font-semibold text-gray-600 shadow-sm">
            <div className="w-2 h-2 rounded-full" style={{ backgroundColor: activeEnv.color }} />
            {activeEnv.name}
          </div>
        )}
        <div className="flex-1" />
        <button
          onClick={() => setEditing('new')}
          className="flex items-center gap-2 px-5 py-2.5 bg-brand-600 hover:bg-brand-700 text-white text-sm font-bold rounded-xl transition-all shadow-lg shadow-brand-200 hover:shadow-brand-300 hover:-translate-y-0.5"
        >
          <Plus className="w-4 h-4" /> New segment
        </button>
      </div>

      {/* Table card */}
      <div className="premium-card shadow-premium-lg border-none bg-white">
        {loading ? (
          <div className="flex items-center justify-center h-40 text-gray-400 text-sm">
            Loading…
          </div>
        ) : error ? (
          <div className="flex items-center justify-center h-40 text-red-500 text-sm">{error}</div>
        ) : segments.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-40 gap-3">
            <Tags className="w-8 h-8 text-gray-200" />
            <p className="text-gray-400 text-sm">
              No segments in {activeEnv?.name ?? 'this environment'} yet.
            </p>
            <button
              onClick={() => setEditing('new')}
              className="flex items-center gap-1.5 text-sm text-brand-600 hover:text-brand-700 font-medium transition-colors"
            >
              <Plus className="w-3.5 h-3.5" /> Create your first segment
            </button>
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-50 bg-gray-50/50">
                  <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">
                    Name
                  </th>
                  <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">
                    Key
                  </th>
                  <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden md:table-cell">
                    Description
                  </th>
                  <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">
                    Rules
                  </th>
                  <th className="px-8 py-4" />
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-50/50">
                {segments.map(seg => (
                  <tr key={seg.key} className="group hover:bg-brand-50/20 transition-all">
                    <td className="px-8 py-5 font-semibold text-gray-900">{seg.name}</td>
                    <td className="px-8 py-5">
                      <span className="font-mono text-brand-600 text-sm">{seg.key}</span>
                    </td>
                    <td className="px-8 py-5 hidden md:table-cell">
                      <span className="text-gray-500 text-xs truncate max-w-xs block">
                        {seg.description ?? <span className="text-gray-300 italic">—</span>}
                      </span>
                    </td>
                    <td className="px-8 py-5">
                      {seg.rules.length > 0 ? (
                        <span className="inline-flex items-center px-2.5 py-0.5 rounded-full text-[10px] font-bold uppercase tracking-wide bg-amber-50 text-amber-700 ring-1 ring-amber-100">
                          {seg.rules.length} rule{seg.rules.length !== 1 ? 's' : ''}
                        </span>
                      ) : (
                        <span className="text-gray-300 text-xs">—</span>
                      )}
                    </td>
                    <td className="px-8 py-5">
                      <div className="flex items-center gap-1 justify-end opacity-0 group-hover:opacity-100 transition-all">
                        <button
                          onClick={() => setEditing(seg)}
                          className="p-1.5 rounded-md text-gray-400 hover:text-gray-700 hover:bg-gray-100 transition-colors"
                          aria-label="Edit segment"
                        >
                          <Pencil className="w-3.5 h-3.5" />
                        </button>
                        <button
                          onClick={() => void handleDelete(seg.key)}
                          className="p-1.5 rounded-md text-gray-400 hover:text-red-500 hover:bg-red-50 transition-colors"
                          aria-label="Delete segment"
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

        {!loading && !error && segments.length > 0 && (
          <div className="px-6 py-3 border-t border-gray-100 text-xs text-gray-400 bg-gray-50">
            {segments.length} segment{segments.length !== 1 ? 's' : ''}
            {activeEnv && ` in ${activeEnv.name}`}
          </div>
        )}
      </div>
    </div>
  )
}
