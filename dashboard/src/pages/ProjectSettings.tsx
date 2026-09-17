import { useState, useEffect, useCallback } from 'react'
import { useParams, useNavigate, useSearchParams } from 'react-router-dom'
import { Key, Users, Settings, Plus, Trash2, Copy, Check, AlertCircle, X, ChevronDown } from 'lucide-react'
import { projectsApi, keysApi, userApi, type ProjectSummary, type ProjectMemberInfo, type SdkKeyInfo, type NewKeyResponse } from '../api'
import { useProject } from '../context/ProjectContext'
import { useEnvironment } from '../context/EnvironmentContext'

type Tab = 'members' | 'keys' | 'settings'

// ---------------------------------------------------------------------------
// SDK Keys tab
// ---------------------------------------------------------------------------

function SdkKeysTab({ projectId }: { projectId: string }) {
  const { environments } = useEnvironment()
  const [keys, setKeys] = useState<SdkKeyInfo[]>([])
  const [loadError, setLoadError] = useState('')
  const [newKeyName, setNewKeyName] = useState('')
  const [newKeyEnvId, setNewKeyEnvId] = useState('')
  const [creating, setCreating] = useState(false)
  const [showCreateForm, setShowCreateForm] = useState(false)
  const [revealedKey, setRevealedKey] = useState<NewKeyResponse | null>(null)
  const [copiedId, setCopiedId] = useState<number | null>(null)
  const [revoking, setRevoking] = useState<number | null>(null)

  const load = useCallback(async () => {
    setLoadError('')
    try {
      setKeys(await keysApi.list(projectId))
    } catch {
      setLoadError('Failed to load SDK keys.')
    }
  }, [projectId])

  useEffect(() => { void load() }, [load])

  useEffect(() => {
    if (!newKeyEnvId && environments.length > 0) {
      setNewKeyEnvId(environments[0].id)
    }
  }, [environments, newKeyEnvId])

  async function handleCreate() {
    if (!newKeyName.trim() || !newKeyEnvId) return
    setCreating(true)
    try {
      const created = await keysApi.create(projectId, newKeyName.trim(), newKeyEnvId)
      setRevealedKey(created)
      setNewKeyName('')
      setShowCreateForm(false)
      await load()
    } finally {
      setCreating(false)
    }
  }

  async function handleRevoke(id: number) {
    if (!confirm('Revoke this key? Any SDK clients using it will stop working immediately.')) return
    setRevoking(id)
    try {
      await keysApi.revoke(projectId, id)
      await load()
    } finally {
      setRevoking(null)
    }
  }

  async function handleCopy(key: string, id: number) {
    await navigator.clipboard.writeText(key)
    setCopiedId(id)
    setTimeout(() => setCopiedId(null), 2000)
  }

  return (
    <div>
      {loadError && <p className="text-rose-400 text-sm mb-4">{loadError}</p>}

      {revealedKey && (
        <div className="mb-4 p-4 rounded-lg bg-brand-50 border border-brand-200">
          <div className="flex items-start justify-between gap-2 mb-2">
            <p className="text-brand-700 text-xs font-medium flex items-center gap-1.5">
              <AlertCircle className="w-3.5 h-3.5" />
              Save this key now — it won't be shown again
            </p>
            <button onClick={() => setRevealedKey(null)} className="text-gray-400 hover:text-gray-600">
              <X className="w-3.5 h-3.5" />
            </button>
          </div>
          <div className="flex items-center gap-2 p-2.5 rounded bg-white border border-gray-200">
            <code className="flex-1 text-brand-600 text-xs font-mono break-all">{revealedKey.key}</code>
            <button
              onClick={() => void handleCopy(revealedKey.key, revealedKey.id)}
              className="shrink-0 p-1.5 rounded bg-gray-100 hover:bg-gray-200 text-gray-500 hover:text-gray-800 transition-colors"
            >
              {copiedId === revealedKey.id
                ? <Check className="w-3.5 h-3.5 text-brand-400" />
                : <Copy className="w-3.5 h-3.5" />}
            </button>
          </div>
        </div>
      )}

      <div className="space-y-2 mb-4">
        {keys.map(k => (
          <div key={k.id} className="flex items-center justify-between gap-3 p-3 rounded-lg bg-gray-50 border border-gray-100">
            <div className="min-w-0">
              <p className="text-gray-900 text-sm font-medium truncate">{k.name}</p>
              <p className="text-gray-500 text-xs font-mono">{k.prefix}… · {k.environment_name}</p>
            </div>
            <div className="flex items-center gap-2 shrink-0">
              <p className="text-gray-400 text-xs hidden sm:block">
                {new Date(k.created_at).toLocaleDateString()}
              </p>
              <button
                onClick={() => void handleRevoke(k.id)}
                disabled={revoking === k.id || keys.length <= 1}
                title={keys.length <= 1 ? 'Cannot revoke the last key' : 'Revoke key'}
                className="p-1.5 rounded bg-gray-100 hover:bg-rose-50 text-gray-400 hover:text-rose-500 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
              >
                {revoking === k.id
                  ? <span className="inline-block w-3.5 h-3.5 border border-gray-400 border-t-transparent rounded-full animate-spin" />
                  : <Trash2 className="w-3.5 h-3.5" />}
              </button>
            </div>
          </div>
        ))}
        {keys.length === 0 && !loadError && (
          <p className="text-gray-400 text-sm text-center py-4">No keys yet.</p>
        )}
      </div>

      {showCreateForm ? (
        <div className="space-y-2">
          <div className="flex gap-2">
            <input
              autoFocus
              type="text"
              value={newKeyName}
              onChange={e => setNewKeyName(e.target.value)}
              onKeyDown={e => { if (e.key === 'Escape') setShowCreateForm(false) }}
              placeholder="Key name (e.g. iOS Production)"
              className="flex-1 bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
            />
            <div className="relative">
              <select
                value={newKeyEnvId}
                onChange={e => setNewKeyEnvId(e.target.value)}
                className="appearance-none bg-white border border-gray-100 rounded-xl px-3 py-2.5 pr-7 text-sm text-gray-700 focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
              >
                {environments.map(env => (
                  <option key={env.id} value={env.id}>{env.name}</option>
                ))}
              </select>
              <ChevronDown className="absolute right-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-gray-400 pointer-events-none" />
            </div>
          </div>
          <div className="flex gap-2">
            <button
              onClick={() => void handleCreate()}
              disabled={creating || !newKeyName.trim() || !newKeyEnvId}
              className="px-6 py-2.5 bg-brand-600 hover:bg-brand-700 disabled:opacity-50 text-white text-sm font-bold rounded-xl transition-all shadow-lg shadow-brand-200"
            >
              {creating ? '…' : 'Create'}
            </button>
            <button
              onClick={() => { setShowCreateForm(false); setNewKeyName('') }}
              className="px-3 py-2 bg-gray-100 hover:bg-gray-200 text-gray-600 text-sm rounded-lg transition-colors"
            >
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <button
          onClick={() => setShowCreateForm(true)}
          className="flex items-center gap-2 px-3 py-2 bg-white border border-gray-200 hover:bg-gray-50 text-gray-700 text-sm font-medium rounded-lg transition-colors shadow-sm"
        >
          <Plus className="w-3.5 h-3.5" /> Generate new key
        </button>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Members tab
// ---------------------------------------------------------------------------

function MembersTab({ projectId }: { projectId: string }) {
  const [members, setMembers] = useState<ProjectMemberInfo[]>([])
  const [allUsers, setAllUsers] = useState<{ id: number; name: string; email: string }[]>([])
  const [loadError, setLoadError] = useState('')
  const [showAddForm, setShowAddForm] = useState(false)
  const [addUserId, setAddUserId] = useState<number | ''>('')
  const [addRole, setAddRole] = useState('viewer')
  const [adding, setAdding] = useState(false)
  const [updatingId, setUpdatingId] = useState<number | null>(null)
  const [removingId, setRemovingId] = useState<number | null>(null)

  const load = useCallback(async () => {
    setLoadError('')
    try {
      const [m, u] = await Promise.all([
        projectsApi.listMembers(projectId),
        userApi.list(),
      ])
      setMembers(m)
      setAllUsers(u.map(u => ({ id: u.id, name: u.name, email: u.email })))
    } catch {
      setLoadError('Failed to load members.')
    }
  }, [projectId])

  useEffect(() => { void load() }, [load])

  const memberIds = new Set(members.map(m => m.user_id))
  const availableUsers = allUsers.filter(u => !memberIds.has(u.id))

  async function handleAdd() {
    if (addUserId === '') return
    setAdding(true)
    try {
      await projectsApi.addMember(projectId, addUserId, addRole)
      setShowAddForm(false)
      setAddUserId('')
      setAddRole('viewer')
      await load()
    } finally {
      setAdding(false)
    }
  }

  async function handleRoleChange(userId: number, role: string) {
    setUpdatingId(userId)
    try {
      await projectsApi.updateMemberRole(projectId, userId, role)
      await load()
    } finally {
      setUpdatingId(null)
    }
  }

  async function handleRemove(userId: number) {
    if (!confirm('Remove this member from the project?')) return
    setRemovingId(userId)
    try {
      await projectsApi.removeMember(projectId, userId)
      await load()
    } finally {
      setRemovingId(null)
    }
  }

  return (
    <div>
      {loadError && <p className="text-rose-400 text-sm mb-4">{loadError}</p>}

      <div className="space-y-2 mb-4">
        {members.map(m => (
          <div key={m.user_id} className="flex items-center gap-3 p-3 rounded-lg bg-gray-50 border border-gray-100">
            <div className="w-8 h-8 rounded-lg bg-brand-50 flex items-center justify-center shrink-0 text-brand-600 text-sm font-bold">
              {m.name.charAt(0).toUpperCase()}
            </div>
            <div className="flex-1 min-w-0">
              <p className="text-gray-900 text-sm font-medium truncate">{m.name}</p>
              <p className="text-gray-400 text-xs truncate">{m.email}</p>
            </div>
            <div className="relative shrink-0">
              <select
                value={m.role}
                onChange={e => void handleRoleChange(m.user_id, e.target.value)}
                disabled={updatingId === m.user_id}
                className="appearance-none text-xs bg-white border border-gray-200 rounded-lg px-2.5 py-1.5 pr-6 text-gray-700 focus:outline-none focus:ring-2 focus:ring-brand-500/20 disabled:opacity-50"
              >
                <option value="admin">Admin</option>
                <option value="editor">Editor</option>
                <option value="viewer">Viewer</option>
              </select>
              <ChevronDown className="absolute right-1.5 top-1/2 -translate-y-1/2 w-3 h-3 text-gray-400 pointer-events-none" />
            </div>
            <button
              onClick={() => void handleRemove(m.user_id)}
              disabled={removingId === m.user_id}
              className="p-1.5 rounded hover:bg-rose-50 text-gray-400 hover:text-rose-500 disabled:opacity-30 transition-colors"
            >
              <Trash2 className="w-3.5 h-3.5" />
            </button>
          </div>
        ))}
        {members.length === 0 && !loadError && (
          <p className="text-gray-400 text-sm text-center py-4">No members yet.</p>
        )}
      </div>

      {showAddForm ? (
        <div className="flex gap-2 flex-wrap">
          <div className="relative flex-1 min-w-[160px]">
            <select
              value={addUserId}
              onChange={e => setAddUserId(Number(e.target.value))}
              className="w-full appearance-none bg-white border border-gray-100 rounded-xl px-3 py-2.5 pr-7 text-sm text-gray-700 focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
            >
              <option value="">Select user…</option>
              {availableUsers.map(u => (
                <option key={u.id} value={u.id}>{u.name} ({u.email})</option>
              ))}
            </select>
            <ChevronDown className="absolute right-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-gray-400 pointer-events-none" />
          </div>
          <div className="relative">
            <select
              value={addRole}
              onChange={e => setAddRole(e.target.value)}
              className="appearance-none bg-white border border-gray-100 rounded-xl px-3 py-2.5 pr-7 text-sm text-gray-700 focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
            >
              <option value="admin">Admin</option>
              <option value="editor">Editor</option>
              <option value="viewer">Viewer</option>
            </select>
            <ChevronDown className="absolute right-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-gray-400 pointer-events-none" />
          </div>
          <button
            onClick={() => void handleAdd()}
            disabled={adding || addUserId === ''}
            className="px-5 py-2.5 bg-brand-600 hover:bg-brand-700 disabled:opacity-50 text-white text-sm font-bold rounded-xl transition-all shadow-lg shadow-brand-200"
          >
            {adding ? '…' : 'Add'}
          </button>
          <button
            onClick={() => { setShowAddForm(false); setAddUserId('') }}
            className="px-3 py-2 bg-gray-100 hover:bg-gray-200 text-gray-600 text-sm rounded-lg transition-colors"
          >
            Cancel
          </button>
        </div>
      ) : (
        <button
          onClick={() => setShowAddForm(true)}
          disabled={availableUsers.length === 0}
          className="flex items-center gap-2 px-3 py-2 bg-white border border-gray-200 hover:bg-gray-50 text-gray-700 text-sm font-medium rounded-lg transition-colors shadow-sm disabled:opacity-40 disabled:cursor-not-allowed"
        >
          <Plus className="w-3.5 h-3.5" /> Add member
        </button>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Settings tab
// ---------------------------------------------------------------------------

function SettingsTab({ project, onRenamed, onDeleted }: {
  project: ProjectSummary
  onRenamed: (p: ProjectSummary) => void
  onDeleted: () => void
}) {
  const [name, setName] = useState(project.name)
  const [renaming, setRenaming] = useState(false)
  const [renameError, setRenameError] = useState('')
  const [deleting, setDeleting] = useState(false)

  async function handleRename() {
    if (!name.trim() || name.trim() === project.name) return
    setRenaming(true)
    setRenameError('')
    try {
      const updated = await projectsApi.rename(project.id, name.trim())
      onRenamed(updated)
    } catch (e) {
      setRenameError(e instanceof Error ? e.message : 'Rename failed.')
    } finally {
      setRenaming(false)
    }
  }

  async function handleDelete() {
    if (!confirm(`Delete "${project.name}"? This will permanently remove all environments, flags, and SDK keys.`)) return
    setDeleting(true)
    try {
      await projectsApi.delete(project.id)
      onDeleted()
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Delete failed.')
      setDeleting(false)
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <label className="block text-sm font-medium text-gray-700 mb-1.5">Project name</label>
        {renameError && <p className="mb-2 text-xs text-rose-500">{renameError}</p>}
        <div className="flex gap-2">
          <input
            type="text"
            value={name}
            onChange={e => setName(e.target.value)}
            className="flex-1 bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
          />
          <button
            onClick={() => void handleRename()}
            disabled={renaming || !name.trim() || name.trim() === project.name}
            className="px-4 py-2.5 bg-brand-600 hover:bg-brand-700 disabled:opacity-50 text-white text-sm font-bold rounded-xl transition-all shadow-lg shadow-brand-200"
          >
            {renaming ? '…' : 'Rename'}
          </button>
        </div>
        <p className="mt-1.5 text-xs text-gray-400">Slug: {project.slug}</p>
      </div>

      <div className="pt-4 border-t border-red-100">
        <h3 className="text-sm font-semibold text-rose-600 mb-2">Danger zone</h3>
        <p className="text-xs text-gray-500 mb-3">
          Deleting this project permanently removes all environments, flags, impressions, and SDK keys.
          This cannot be undone.
        </p>
        <button
          onClick={() => void handleDelete()}
          disabled={deleting}
          className="flex items-center gap-2 px-4 py-2 bg-rose-50 hover:bg-rose-100 text-rose-600 text-sm font-semibold rounded-lg transition-colors border border-rose-200 disabled:opacity-50"
        >
          <Trash2 className="w-3.5 h-3.5" />
          {deleting ? 'Deleting…' : 'Delete project'}
        </button>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

const TABS: { id: Tab; label: string; icon: React.ElementType }[] = [
  { id: 'members', label: 'Members', icon: Users },
  { id: 'keys', label: 'SDK Keys', icon: Key },
  { id: 'settings', label: 'Settings', icon: Settings },
]

const VALID_TABS: Tab[] = ['members', 'keys', 'settings']

export default function ProjectSettings() {
  const { projectId } = useParams<{ projectId: string }>()
  const navigate = useNavigate()
  const { reload: reloadContext } = useProject()
  const [searchParams] = useSearchParams()
  const [project, setProject] = useState<ProjectSummary | null>(null)
  const [loading, setLoading] = useState(true)
  // Deep-linkable via ?tab=keys (e.g. from Settings' "get an SDK key" pointer).
  const requestedTab = searchParams.get('tab')
  const [tab, setTab] = useState<Tab>(
    VALID_TABS.includes(requestedTab as Tab) ? (requestedTab as Tab) : 'members',
  )

  useEffect(() => {
    if (!projectId) return
    projectsApi.list()
      .then(list => setProject(list.find(p => p.id === projectId) ?? null))
      .catch(() => setProject(null))
      .finally(() => setLoading(false))
  }, [projectId])

  function handleDeleted() {
    void reloadContext()
    navigate('/projects')
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center py-16">
        <span className="inline-block w-5 h-5 border-2 border-gray-200 border-t-brand-500 rounded-full animate-spin" />
      </div>
    )
  }

  if (!project || !projectId) {
    return <p className="text-gray-400 text-sm">Project not found.</p>
  }

  return (
    <div className="w-full max-w-4xl">
      <div className="mb-6">
        <button
          onClick={() => navigate('/projects')}
          className="text-xs text-gray-400 hover:text-gray-600 mb-2 transition-colors"
        >
          ← Projects
        </button>
        <h1 className="text-xl font-bold text-gray-900">{project.name}</h1>
        <p className="text-gray-400 text-xs font-mono mt-0.5">{project.slug}</p>
      </div>

      {/* Tab bar */}
      <div className="flex gap-1 mb-6 p-1 bg-gray-100 rounded-xl w-fit">
        {TABS.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            onClick={() => setTab(id)}
            className={`flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-semibold transition-all ${
              tab === id
                ? 'bg-white text-gray-900 shadow-sm'
                : 'text-gray-500 hover:text-gray-700'
            }`}
          >
            <Icon className="w-3.5 h-3.5" />
            {label}
          </button>
        ))}
      </div>

      <div className="premium-card shadow-premium-lg border-none p-6">
        {tab === 'members' && <MembersTab projectId={projectId} />}
        {tab === 'keys' && <SdkKeysTab projectId={projectId} />}
        {tab === 'settings' && (
          <SettingsTab
            project={project}
            onRenamed={p => { setProject(p); void reloadContext() }}
            onDeleted={handleDeleted}
          />
        )}
      </div>
    </div>
  )
}
