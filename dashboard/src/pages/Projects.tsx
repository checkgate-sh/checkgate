import { useState, useEffect, useCallback } from 'react'
import { Plus, FolderKanban, Trash2, Settings } from 'lucide-react'
import { useNavigate } from 'react-router-dom'
import { projectsApi, type ProjectSummary } from '../api'
import { useProject } from '../context/ProjectContext'

function slugify(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '')
}

function NewProjectModal({ onClose, onCreated }: {
  onClose: () => void
  onCreated: (p: ProjectSummary) => void
}) {
  const [name, setName] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')

  async function handleCreate() {
    if (!name.trim()) return
    setLoading(true)
    setError('')
    try {
      const project = await projectsApi.create(name.trim())
      onCreated(project)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to create project.')
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/20 backdrop-blur-sm">
      <div className="bg-white rounded-2xl shadow-2xl w-full max-w-md mx-4 p-6">
        <h2 className="text-lg font-bold text-gray-900 mb-4">New project</h2>
        {error && <p className="mb-3 text-sm text-rose-500">{error}</p>}
        <div className="mb-4">
          <label className="block text-sm font-medium text-gray-700 mb-1.5">Project name</label>
          <input
            autoFocus
            type="text"
            value={name}
            onChange={e => setName(e.target.value)}
            onKeyDown={e => { if (e.key === 'Enter') void handleCreate(); if (e.key === 'Escape') onClose() }}
            placeholder="My App"
            className="w-full bg-white border border-gray-200 rounded-xl px-4 py-2.5 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all"
          />
          {name.trim() && (
            <p className="mt-1.5 text-xs text-gray-400">Slug: {slugify(name)}</p>
          )}
        </div>
        <div className="flex gap-3">
          <button
            onClick={onClose}
            className="flex-1 py-2.5 px-4 bg-gray-100 hover:bg-gray-200 text-gray-600 font-medium rounded-xl text-sm transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={() => void handleCreate()}
            disabled={loading || !name.trim()}
            className="flex-1 py-2.5 px-4 bg-brand-600 hover:bg-brand-700 disabled:opacity-50 text-white font-bold rounded-xl text-sm transition-all shadow-lg shadow-brand-200"
          >
            {loading ? 'Creating…' : 'Create project'}
          </button>
        </div>
      </div>
    </div>
  )
}

export default function Projects() {
  const [projects, setProjects] = useState<ProjectSummary[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [showModal, setShowModal] = useState(false)
  const navigate = useNavigate()
  const { reload: reloadContext } = useProject()

  const load = useCallback(async () => {
    setError('')
    try {
      setProjects(await projectsApi.list())
    } catch {
      setError('Failed to load projects.')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void load() }, [load])

  async function handleDelete(p: ProjectSummary) {
    if (projects.length <= 1) return
    if (!confirm(`Delete "${p.name}"? This will permanently remove all environments, flags, and SDK keys in this project.`)) return
    try {
      await projectsApi.delete(p.id)
      await load()
      await reloadContext()
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Delete failed.')
    }
  }

  async function handleCreated(p: ProjectSummary) {
    setShowModal(false)
    await load()
    await reloadContext()
    navigate(`/projects/${p.id}`)
  }

  return (
    <div className="w-full">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-xl font-bold text-gray-900">Projects</h1>
          <p className="text-gray-500 text-sm mt-0.5">Each project has its own environments, flags, and SDK keys.</p>
        </div>
        <button
          onClick={() => setShowModal(true)}
          className="flex items-center gap-2 px-4 py-2.5 bg-brand-600 hover:bg-brand-700 text-white font-bold text-sm rounded-xl transition-all shadow-lg shadow-brand-200 hover:shadow-brand-300 hover:-translate-y-0.5"
        >
          <Plus className="w-4 h-4" /> New project
        </button>
      </div>

      {error && <p className="text-rose-500 text-sm mb-4">{error}</p>}

      {loading ? (
        <div className="flex items-center justify-center py-16">
          <span className="inline-block w-5 h-5 border-2 border-gray-200 border-t-brand-500 rounded-full animate-spin" />
        </div>
      ) : (
        <div className="premium-card shadow-premium-lg border-none overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-50 bg-gray-50/50">
                <th className="px-5 py-3 text-left text-xs font-semibold text-gray-500 uppercase tracking-wide">Project</th>
                <th className="px-5 py-3 text-left text-xs font-semibold text-gray-500 uppercase tracking-wide hidden sm:table-cell">Environments</th>
                <th className="px-5 py-3 text-left text-xs font-semibold text-gray-500 uppercase tracking-wide hidden sm:table-cell">Members</th>
                <th className="px-5 py-3 text-left text-xs font-semibold text-gray-500 uppercase tracking-wide hidden md:table-cell">Created</th>
                <th className="px-5 py-3" />
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-50">
              {projects.map(p => (
                <tr key={p.id} className="hover:bg-gray-50/50 transition-colors">
                  <td className="px-5 py-4">
                    <div className="flex items-center gap-3">
                      <div className="w-8 h-8 rounded-lg bg-brand-50 flex items-center justify-center shrink-0">
                        <FolderKanban className="w-4 h-4 text-brand-600" />
                      </div>
                      <div>
                        <p className="font-semibold text-gray-900">{p.name}</p>
                        <p className="text-xs text-gray-400 font-mono">{p.slug}</p>
                      </div>
                    </div>
                  </td>
                  <td className="px-5 py-4 text-gray-500 hidden sm:table-cell">{p.environment_count}</td>
                  <td className="px-5 py-4 text-gray-500 hidden sm:table-cell">{p.member_count}</td>
                  <td className="px-5 py-4 text-gray-400 text-xs hidden md:table-cell">
                    {new Date(p.created_at).toLocaleDateString()}
                  </td>
                  <td className="px-5 py-4">
                    <div className="flex items-center gap-1 justify-end">
                      <button
                        onClick={() => navigate(`/projects/${p.id}`)}
                        className="p-1.5 rounded-lg hover:bg-gray-100 text-gray-400 hover:text-gray-700 transition-colors"
                        title="Settings"
                      >
                        <Settings className="w-4 h-4" />
                      </button>
                      <button
                        onClick={() => void handleDelete(p)}
                        disabled={projects.length <= 1}
                        title={projects.length <= 1 ? 'Cannot delete the only project' : 'Delete project'}
                        className="p-1.5 rounded-lg hover:bg-rose-50 text-gray-400 hover:text-rose-500 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
                      >
                        <Trash2 className="w-4 h-4" />
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
              {projects.length === 0 && (
                <tr>
                  <td colSpan={5} className="px-5 py-12 text-center text-gray-400 text-sm">
                    No projects yet.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      )}

      {showModal && (
        <NewProjectModal
          onClose={() => setShowModal(false)}
          onCreated={p => void handleCreated(p)}
        />
      )}
    </div>
  )
}
