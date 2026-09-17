import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Globe, GitCompare, Plus, Trash2, Star, AlertCircle, X, ShieldCheck, Shield } from 'lucide-react'
import { useEnvironment, type Environment } from '../context/EnvironmentContext'
import { useAuth } from '../context/AuthContext'
import { useProject } from '../context/ProjectContext'

// ---------------------------------------------------------------------------
// Color picker options
// ---------------------------------------------------------------------------

const COLORS = [
  { label: 'Red',    value: '#ef4444' },
  { label: 'Amber',  value: '#f59e0b' },
  { label: 'Cyan',   value: '#06b6d4' },
  { label: 'Blue',   value: '#3b82f6' },
  { label: 'Purple', value: '#8b5cf6' },
  { label: 'Indigo', value: '#4f46e5' },
  { label: 'Pink',   value: '#ec4899' },
  { label: 'Gray',   value: '#6b7280' },
]

// ---------------------------------------------------------------------------
// Create form
// ---------------------------------------------------------------------------

function CreateEnvironmentForm({ onDone }: { onDone: () => void }) {
  const { reload } = useEnvironment()
  const { activeProject } = useProject()
  const [name, setName] = useState('')
  const [slug, setSlug] = useState('')
  const [slugTouched, setSlugTouched] = useState(false)
  const [color, setColor] = useState('#4f46e5')
  const [error, setError] = useState('')
  const [saving, setSaving] = useState(false)

  function deriveSlug(n: string) {
    return n.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-]/g, '')
  }

  function handleNameChange(val: string) {
    setName(val)
    if (!slugTouched) setSlug(deriveSlug(val))
  }

  async function handleCreate() {
    setError('')
    if (!name.trim() || !slug.trim() || !activeProject) return
    setSaving(true)
    try {
      const res = await fetch(`/api/projects/${activeProject.id}/environments`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json', 'X-Checkgate-Request': '1' },
        body: JSON.stringify({ name: name.trim(), slug: slug.trim(), color }),
      })
      if (res.status === 409) {
        setError('A slug with that name already exists.')
        return
      }
      if (res.status === 422) {
        setError('Slug must be lowercase letters, numbers, and hyphens only.')
        return
      }
      if (!res.ok) {
        setError(`Server returned ${res.status}.`)
        return
      }
      await reload()
      onDone()
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="bg-gray-50 border border-gray-200 rounded-2xl p-5 space-y-4">
      <div className="flex items-center justify-between">
        <p className="text-sm font-semibold text-gray-700">New environment</p>
        <button onClick={onDone} className="text-gray-400 hover:text-gray-600"><X className="w-4 h-4" /></button>
      </div>

      {error && (
        <div className="flex items-center gap-2 p-3 rounded-lg bg-rose-50 border border-rose-100 text-rose-600 text-xs">
          <AlertCircle className="w-3.5 h-3.5 shrink-0" /> {error}
        </div>
      )}

      <div className="grid grid-cols-2 gap-3">
        <div className="col-span-2">
          <label className="block text-xs font-medium text-gray-600 mb-1">Name</label>
          <input
            autoFocus
            type="text"
            value={name}
            onChange={e => handleNameChange(e.target.value)}
            placeholder="e.g. Canary"
            className="w-full bg-white border border-gray-200 rounded-lg px-3 py-2 text-sm text-gray-900 placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-brand-500/20 focus:border-brand-400"
          />
        </div>
        <div>
          <label className="block text-xs font-medium text-gray-600 mb-1">Slug</label>
          <input
            type="text"
            value={slug}
            onChange={e => { setSlug(e.target.value); setSlugTouched(true) }}
            placeholder="canary"
            className="w-full bg-white border border-gray-200 rounded-lg px-3 py-2 text-sm font-mono text-gray-900 placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-brand-500/20 focus:border-brand-400"
          />
        </div>
        <div>
          <label className="block text-xs font-medium text-gray-600 mb-1">Color</label>
          <div className="flex flex-wrap gap-1.5 pt-1">
            {COLORS.map(c => (
              <button
                key={c.value}
                type="button"
                onClick={() => setColor(c.value)}
                title={c.label}
                className={`w-6 h-6 rounded-full transition-all ${color === c.value ? 'ring-2 ring-offset-1 ring-gray-400 scale-110' : 'hover:scale-110'}`}
                style={{ backgroundColor: c.value }}
              />
            ))}
          </div>
        </div>
      </div>

      <div className="flex gap-2 pt-1">
        <button
          onClick={() => void handleCreate()}
          disabled={saving || !name.trim() || !slug.trim()}
          className="flex-1 py-2 px-4 bg-brand-600 hover:bg-brand-700 disabled:opacity-40 disabled:cursor-not-allowed text-white text-sm font-bold rounded-lg transition-all"
        >
          {saving ? 'Creating…' : 'Create environment'}
        </button>
        <button
          onClick={onDone}
          className="px-3 py-2 bg-white border border-gray-200 hover:bg-gray-50 text-gray-600 text-sm rounded-lg transition-colors"
        >
          Cancel
        </button>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Environment row
// ---------------------------------------------------------------------------

function EnvRow({ env, onDelete, onSetDefault, onToggleApproval }: {
  env: Environment
  onDelete: (id: string) => void
  onSetDefault: (id: string) => void
  onToggleApproval: (id: string, next: boolean) => void
}) {
  const { session } = useAuth()
  const isAdmin = session?.user.role === 'admin'

  return (
    <div className="flex items-center gap-4 p-4 rounded-xl bg-white border border-gray-100 shadow-sm">
      {/* Color dot */}
      <div className="w-3 h-3 rounded-full shrink-0" style={{ backgroundColor: env.color }} />

      {/* Info */}
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <p className="text-sm font-semibold text-gray-900">{env.name}</p>
          {env.is_default && (
            <span className="px-1.5 py-0.5 bg-brand-50 text-brand-700 text-[10px] font-bold rounded uppercase tracking-wide border border-brand-100">
              default
            </span>
          )}
          {env.require_approval && (
            <span className="flex items-center gap-1 px-1.5 py-0.5 bg-amber-50 text-amber-700 text-[10px] font-bold rounded uppercase tracking-wide border border-amber-100">
              <ShieldCheck className="w-2.5 h-2.5" /> approval required
            </span>
          )}
        </div>
        <p className="text-xs text-gray-400 font-mono">{env.slug}</p>
      </div>

      {/* Actions */}
      {isAdmin && (
        <div className="flex items-center gap-1 shrink-0">
          <button
            onClick={() => onToggleApproval(env.id, !env.require_approval)}
            title={env.require_approval ? 'Disable approval requirement' : 'Require approval for flag changes'}
            className={`p-1.5 rounded-lg transition-colors ${
              env.require_approval
                ? 'text-amber-500 hover:text-amber-600 hover:bg-amber-50'
                : 'text-gray-300 hover:text-amber-500 hover:bg-amber-50'
            }`}
          >
            {env.require_approval ? <ShieldCheck className="w-3.5 h-3.5" /> : <Shield className="w-3.5 h-3.5" />}
          </button>
          {!env.is_default && (
            <button
              onClick={() => onSetDefault(env.id)}
              title="Set as default"
              className="p-1.5 rounded-lg text-gray-300 hover:text-amber-500 hover:bg-amber-50 transition-colors"
            >
              <Star className="w-3.5 h-3.5" />
            </button>
          )}
          <button
            onClick={() => onDelete(env.id)}
            disabled={env.is_default}
            title={env.is_default ? 'Cannot delete the default environment' : 'Delete environment'}
            className="p-1.5 rounded-lg text-gray-300 hover:text-rose-500 hover:bg-rose-50 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
          >
            <Trash2 className="w-3.5 h-3.5" />
          </button>
        </div>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function Environments() {
  const { environments, reload } = useEnvironment()
  const { session } = useAuth()
  const { activeProject } = useProject()
  const isAdmin = session?.user.role === 'admin'
  const [showCreate, setShowCreate] = useState(false)
  const [deleteError, setDeleteError] = useState('')

  async function handleDelete(id: string) {
    if (!activeProject) return
    if (!confirm('Delete this environment? All flags scoped to it will also be deleted.')) return
    setDeleteError('')
    const res = await fetch(`/api/projects/${activeProject.id}/environments/${id}`, {
      method: 'DELETE',
      credentials: 'same-origin',
      headers: { 'X-Checkgate-Request': '1' },
    })
    if (res.status === 422) {
      setDeleteError('Cannot delete the default environment or the last environment.')
      return
    }
    if (!res.ok) {
      setDeleteError(`Delete failed (${res.status}).`)
      return
    }
    await reload()
  }

  async function handleSetDefault(id: string) {
    if (!activeProject) return
    const res = await fetch(`/api/projects/${activeProject.id}/environments/${id}/default`, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'X-Checkgate-Request': '1' },
    })
    if (res.ok) await reload()
  }

  async function handleToggleApproval(id: string, next: boolean) {
    if (!activeProject) return
    const res = await fetch(`/api/projects/${activeProject.id}/environments/${id}/require-approval`, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json', 'X-Checkgate-Request': '1' },
      body: JSON.stringify({ require_approval: next }),
    })
    if (res.ok) await reload()
  }

  return (
    <div className="w-full space-y-5">
      <div className="premium-card shadow-premium-lg border-none">
        {/* Header */}
        <div className="flex items-start gap-5 px-6 py-5 border-b border-gray-50 bg-white">
          <div className="w-10 h-10 rounded-xl bg-brand-50 flex items-center justify-center shrink-0">
            <Globe className="w-5 h-5 text-brand-600" />
          </div>
          <div className="flex-1">
            <h2 className="text-gray-900 font-display font-bold text-sm tracking-tight">Environments</h2>
            <p className="text-gray-400 text-xs mt-0.5">Isolate flag configurations across production, staging, UAT, and development.</p>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            {environments.length >= 2 && (
              <Link
                to="/environments/diff"
                className="flex items-center gap-1.5 px-3 py-1.5 bg-white border border-gray-200 hover:bg-gray-50 text-gray-600 text-xs font-bold rounded-lg transition-all"
              >
                <GitCompare className="w-3.5 h-3.5" /> Compare
              </Link>
            )}
            {isAdmin && !showCreate && (
              <button
                onClick={() => setShowCreate(true)}
                className="flex items-center gap-1.5 px-3 py-1.5 bg-brand-600 hover:bg-brand-700 text-white text-xs font-bold rounded-lg transition-all shadow-sm shadow-brand-200"
              >
                <Plus className="w-3.5 h-3.5" /> New
              </button>
            )}
          </div>
        </div>

        <div className="p-6 space-y-3">
          {deleteError && (
            <div className="flex items-center gap-2 p-3 rounded-lg bg-rose-50 border border-rose-100 text-rose-600 text-xs">
              <AlertCircle className="w-3.5 h-3.5 shrink-0" /> {deleteError}
            </div>
          )}

          {showCreate && (
            <CreateEnvironmentForm onDone={() => setShowCreate(false)} />
          )}

          {environments.map(env => (
            <EnvRow
              key={env.id}
              env={env}
              onDelete={id => void handleDelete(id)}
              onSetDefault={id => void handleSetDefault(id)}
              onToggleApproval={(id, next) => void handleToggleApproval(id, next)}
            />
          ))}

          {environments.length === 0 && (
            <p className="text-center text-gray-400 text-sm py-6">No environments yet.</p>
          )}
        </div>
      </div>
    </div>
  )
}
