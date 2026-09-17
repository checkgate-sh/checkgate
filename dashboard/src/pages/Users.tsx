import { useState, useEffect, useCallback } from 'react'
import { UserPlus, Trash2, Shield, Eye, EyeOff, X, Pencil } from 'lucide-react'
import { useAuth } from '../context/AuthContext'
import { userApi, type ApiUser } from '../api'
import type { UserRole } from '../types'

const inputClass =
  'w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium'

function RoleBadge({ role }: { role: string }) {
  if (role === 'admin') {
    return (
      <span className="inline-flex items-center gap-1.5 px-3 py-1 rounded-full text-[10px] font-bold uppercase tracking-wide bg-brand-50 text-brand-700 ring-1 ring-brand-200">
        <Shield className="w-2 h-2" /> Admin
      </span>
    )
  }
  if (role === 'editor') {
    return (
      <span className="inline-flex items-center gap-1.5 px-3 py-1 rounded-full text-[10px] font-bold uppercase tracking-wide bg-blue-50 text-blue-700 ring-1 ring-blue-200">
        <Pencil className="w-2 h-2" /> Editor
      </span>
    )
  }
  return (
    <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-gray-100 text-gray-600 ring-1 ring-gray-200">
      <Eye className="w-3 h-3" /> Viewer
    </span>
  )
}

interface AddUserModalProps {
  onClose: () => void
  onAdd: (name: string, email: string, role: UserRole, password: string) => Promise<void>
}

function AddUserModal({ onClose, onAdd }: AddUserModalProps) {
  const [name, setName] = useState('')
  const [email, setEmail] = useState('')
  const [role, setRole] = useState<UserRole>('viewer')
  const [password, setPassword] = useState('')
  const [showPassword, setShowPassword] = useState(false)
  const [error, setError] = useState('')
  const [saving, setSaving] = useState(false)

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (!name.trim() || !email.trim()) {
      setError('Name and email are required.')
      return
    }
    if (password.length < 8) {
      setError('Password must be at least 8 characters.')
      return
    }
    setSaving(true)
    try {
      await onAdd(name.trim(), email.trim(), role, password)
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to add user.')
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div className="absolute inset-0 bg-gray-900/40 backdrop-blur-sm" onClick={onClose} />
      <div className="relative bg-white border border-gray-50 shadow-premium-xl rounded-2xl w-full max-w-md">
        <div className="flex items-center justify-between px-6 py-5 border-b border-gray-50">
          <h3 className="text-gray-900 font-display font-bold text-lg">Add user</h3>
          <button
            onClick={onClose}
            className="p-1.5 rounded-lg text-gray-500 hover:text-gray-900 hover:bg-gray-100 transition-colors"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <form onSubmit={e => void handleSubmit(e)} className="p-5 space-y-4">
          {error && (
            <div className="p-3 rounded-lg bg-red-50 border border-red-200 text-red-600 text-sm">
              {error}
            </div>
          )}

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1.5">Full name</label>
            <input
              type="text"
              value={name}
              onChange={e => { setName(e.target.value); setError('') }}
              placeholder="Jane Smith"
              className={inputClass}
              autoFocus
            />
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1.5">Email address</label>
            <input
              type="email"
              value={email}
              onChange={e => { setEmail(e.target.value); setError('') }}
              placeholder="jane@company.com"
              className={inputClass}
            />
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1.5">Password</label>
            <div className="relative">
              <input
                type={showPassword ? 'text' : 'password'}
                value={password}
                onChange={e => { setPassword(e.target.value); setError('') }}
                placeholder="Min. 8 characters"
                className={inputClass}
              />
              <button
                type="button"
                onClick={() => setShowPassword(s => !s)}
                className="absolute right-3 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-700 transition-colors"
                aria-label={showPassword ? 'Hide password' : 'Show password'}
              >
                {showPassword ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
              </button>
            </div>
            <p className="mt-1 text-xs text-gray-400">The user will use this to log in.</p>
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1.5">Role</label>
            <div className="flex gap-2">
              {(['admin', 'editor', 'viewer'] as UserRole[]).map(r => (
                <button
                  key={r}
                  type="button"
                  onClick={() => setRole(r)}
                  className={`flex-1 py-2.5 px-3 rounded-xl border text-sm font-bold transition-all capitalize ${
                    role === r
                      ? 'bg-brand-600 border-brand-600 text-white shadow-md shadow-brand-100'
                      : 'bg-white border-gray-100 text-gray-500 hover:border-gray-200 hover:text-gray-900'
                  }`}
                >
                  {r.charAt(0).toUpperCase() + r.slice(1)}
                </button>
              ))}
            </div>
            <p className="mt-1.5 text-xs text-gray-500">
              {role === 'admin'
                ? 'Full access: flags, environments, users, and SDK keys.'
                : role === 'editor'
                ? 'Can create, edit, and delete flags. Cannot manage users or settings.'
                : 'Read-only access to flags and environments.'}
            </p>
          </div>

          <div className="flex gap-3 pt-1">
            <button
              type="button"
              onClick={onClose}
              className="flex-1 py-2.5 px-4 bg-gray-100 hover:bg-gray-200 text-gray-700 text-sm font-medium rounded-lg transition-colors"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={saving}
              className="flex-1 py-3 px-4 bg-brand-600 hover:bg-brand-700 disabled:opacity-50 text-white text-sm font-bold rounded-xl transition-all shadow-lg shadow-brand-200 hover:shadow-brand-300 hover:-translate-y-0.5"
            >
              {saving ? '…' : 'Add user'}
            </button>
          </div>
        </form>
      </div>
    </div>
  )
}

export default function Users() {
  const { session } = useAuth()
  const [users, setUsers] = useState<ApiUser[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [showModal, setShowModal] = useState(false)

  const isAdmin = session?.user.role === 'admin'

  const load = useCallback(async () => {
    setError('')
    try {
      setUsers(await userApi.list())
    } catch {
      setError('Failed to load users.')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void load() }, [load])

  async function handleAdd(name: string, email: string, role: UserRole, password: string) {
    const created = await userApi.create({ name, email, role, password })
    setUsers(prev => [...prev, created])
  }

  async function handleRemove(user: ApiUser) {
    if (user.email === session?.user.email) {
      alert("You can't remove your own account.")
      return
    }
    if (!confirm('Remove this user? They will be unable to log in.')) return
    try {
      await userApi.remove(user.id)
      setUsers(prev => prev.filter(u => u.id !== user.id))
    } catch (err) {
      alert(err instanceof Error ? err.message : 'Failed to remove user.')
    }
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-40 text-gray-400 text-sm">Loading…</div>
    )
  }

  if (error) {
    return (
      <div className="flex items-center justify-center h-40 text-red-500 text-sm">{error}</div>
    )
  }

  return (
    <div className="w-full space-y-4">
      {showModal && (
        <AddUserModal
          onClose={() => setShowModal(false)}
          onAdd={handleAdd}
        />
      )}

      {/* Toolbar */}
      <div className="flex items-center justify-between">
        <p className="text-gray-500 text-sm">{users.length} user{users.length !== 1 ? 's' : ''}</p>
        {isAdmin && (
          <button
            onClick={() => setShowModal(true)}
            className="flex items-center gap-2 px-5 py-2.5 bg-brand-600 hover:bg-brand-700 text-white text-sm font-bold rounded-xl transition-all shadow-lg shadow-brand-200 hover:shadow-brand-300 hover:-translate-y-0.5"
          >
            <UserPlus className="w-4 h-4" /> Add user
          </button>
        )}
      </div>

      <div className="premium-card shadow-premium-lg border-none bg-white">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-50 bg-gray-50/50">
              <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">User</th>
              <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Role</th>
              <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden sm:table-cell">Joined</th>
              {isAdmin && <th className="px-8 py-4" />}
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-50/50">
            {users.map(user => (
              <tr key={user.id} className="group hover:bg-gray-50 transition-colors">
                <td className="px-5 py-3.5">
                  <div className="flex items-center gap-3">
                    <div className="w-9 h-9 rounded-xl bg-gradient-to-br from-brand-500 to-brand-600 shadow-md shadow-brand-200 flex items-center justify-center shrink-0">
                      <span className="text-white text-sm font-bold">{user.name.charAt(0).toUpperCase()}</span>
                    </div>
                    <div>
                      <p className="text-gray-800 font-medium flex items-center gap-2">
                        {user.name}
                        {user.email === session?.user.email && (
                          <span className="text-xs font-normal text-gray-400">(you)</span>
                        )}
                      </p>
                      <p className="text-gray-500 text-xs">{user.email}</p>
                    </div>
                  </div>
                </td>
                <td className="px-5 py-3.5">
                  <RoleBadge role={user.role} />
                </td>
                <td className="px-5 py-3.5 text-gray-500 text-xs hidden sm:table-cell">
                  {new Date(user.created_at).toLocaleDateString()}
                </td>
                {isAdmin && (
                  <td className="px-5 py-3.5">
                    <div className="flex justify-end opacity-0 group-hover:opacity-100 transition-opacity">
                      <button
                        onClick={() => void handleRemove(user)}
                        disabled={user.email === session?.user.email}
                        className="p-1.5 rounded-md text-gray-400 hover:text-rose-500 hover:bg-rose-50 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
                        aria-label="Remove user"
                      >
                        <Trash2 className="w-3.5 h-3.5" />
                      </button>
                    </div>
                  </td>
                )}
              </tr>
            ))}
          </tbody>
        </table>
        {users.length === 0 && (
          <div className="flex items-center justify-center h-24 text-gray-400 text-sm">
            No users yet.
          </div>
        )}
      </div>
    </div>
  )
}
