import { useState, useEffect, useCallback } from 'react'
import { Globe, LogOut, Shield, Info, Code2, Copy, Check, KeyRound, Plus, Trash2, AlertCircle, X } from 'lucide-react'
import { useNavigate, Link } from 'react-router-dom'
import { useAuth } from '../context/AuthContext'
import { useProject } from '../context/ProjectContext'
import { tokensApi, type TokenInfo, type NewTokenResponse, type TokenScope } from '../api'

function SectionCard({ icon: Icon, title, description, children }: {
  icon: React.ElementType
  title: string
  description: string
  children: React.ReactNode
}) {
  return (
    <div className="premium-card shadow-premium-lg border-none">
      <div className="flex items-start gap-5 px-6 py-5 border-b border-gray-50 bg-white">
        <div className="w-10 h-10 rounded-xl bg-brand-50 flex items-center justify-center shrink-0">
          <Icon className="w-5 h-5 text-brand-600" />
        </div>
        <div>
          <h2 className="text-gray-900 font-display font-bold text-sm tracking-tight">{title}</h2>
          <p className="text-gray-400 text-xs mt-0.5">{description}</p>
        </div>
      </div>
      <div className="p-6">{children}</div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Code snippet block
// ---------------------------------------------------------------------------

function CodeBlock({ code }: { code: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <div className="relative">
      <pre className="bg-gray-950 text-gray-100 text-xs leading-relaxed p-4 rounded-xl overflow-x-auto font-mono">
        {code}
      </pre>
      <button
        onClick={() => {
          navigator.clipboard.writeText(code).then(() => {
            setCopied(true)
            setTimeout(() => setCopied(false), 1500)
          })
        }}
        className="absolute top-2.5 right-2.5 p-1.5 rounded-md bg-white/10 hover:bg-white/20 text-gray-300 transition-colors"
        title="Copy"
      >
        {copied ? <Check className="w-3.5 h-3.5 text-brand-400" /> : <Copy className="w-3.5 h-3.5" />}
      </button>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Personal access tokens
// ---------------------------------------------------------------------------

function PersonalAccessTokensSection() {
  const [tokens, setTokens] = useState<TokenInfo[]>([])
  const [loadError, setLoadError] = useState('')
  const [showCreateForm, setShowCreateForm] = useState(false)
  const [newName, setNewName] = useState('')
  const [newScope, setNewScope] = useState<TokenScope>('read_only')
  const [newExpiresInDays, setNewExpiresInDays] = useState<number | null>(90)
  const [creating, setCreating] = useState(false)
  const [revealedToken, setRevealedToken] = useState<NewTokenResponse | null>(null)
  const [copied, setCopied] = useState(false)
  const [revokingId, setRevokingId] = useState<number | null>(null)

  const load = useCallback(async () => {
    setLoadError('')
    try {
      setTokens(await tokensApi.list())
    } catch {
      setLoadError('Failed to load personal access tokens.')
    }
  }, [])

  useEffect(() => { void load() }, [load])

  async function handleCreate() {
    if (!newName.trim()) return
    setCreating(true)
    try {
      const created = await tokensApi.create(newName.trim(), newScope, newExpiresInDays)
      setRevealedToken(created)
      setNewName('')
      setShowCreateForm(false)
      await load()
    } finally {
      setCreating(false)
    }
  }

  async function handleRevoke(id: number) {
    if (!confirm('Revoke this token? Anything using it will stop working immediately.')) return
    setRevokingId(id)
    try {
      await tokensApi.revoke(id)
      await load()
    } finally {
      setRevokingId(null)
    }
  }

  async function handleCopy(token: string) {
    await navigator.clipboard.writeText(token)
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }

  return (
    <SectionCard
      icon={KeyRound}
      title="Personal access tokens"
      description="Scoped, revocable credentials for CI/CD, Terraform, and scripts — act as you, not as an admin-equivalent SDK key."
    >
      {loadError && <p className="text-rose-400 text-sm mb-4">{loadError}</p>}

      {revealedToken && (
        <div className="mb-4 p-4 rounded-lg bg-brand-50 border border-brand-200">
          <div className="flex items-start justify-between gap-2 mb-2">
            <p className="text-brand-700 text-xs font-medium flex items-center gap-1.5">
              <AlertCircle className="w-3.5 h-3.5" />
              Save this token now — it won't be shown again
            </p>
            <button onClick={() => setRevealedToken(null)} className="text-gray-400 hover:text-gray-600">
              <X className="w-3.5 h-3.5" />
            </button>
          </div>
          <div className="flex items-center gap-2 p-2.5 rounded bg-white border border-gray-200">
            <code className="flex-1 text-brand-600 text-xs font-mono break-all">{revealedToken.token}</code>
            <button
              onClick={() => void handleCopy(revealedToken.token)}
              className="shrink-0 p-1.5 rounded bg-gray-100 hover:bg-gray-200 text-gray-500 hover:text-gray-800 transition-colors"
            >
              {copied ? <Check className="w-3.5 h-3.5 text-brand-400" /> : <Copy className="w-3.5 h-3.5" />}
            </button>
          </div>
        </div>
      )}

      <div className="space-y-2 mb-4">
        {tokens.map(t => (
          <div key={t.id} className="flex items-center justify-between gap-3 p-3 rounded-lg bg-gray-50 border border-gray-100">
            <div className="min-w-0">
              <p className="text-gray-900 text-sm font-medium truncate">{t.name}</p>
              <p className="text-gray-500 text-xs font-mono">
                {t.prefix} · {t.scope === 'read_only' ? 'read-only' : 'read-write'}
                {t.expires_at ? ` · expires ${new Date(t.expires_at).toLocaleDateString()}` : ' · never expires'}
              </p>
            </div>
            <button
              onClick={() => void handleRevoke(t.id)}
              disabled={revokingId === t.id}
              className="shrink-0 p-1.5 rounded bg-gray-100 hover:bg-rose-50 text-gray-400 hover:text-rose-500 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
            >
              {revokingId === t.id
                ? <span className="inline-block w-3.5 h-3.5 border border-gray-400 border-t-transparent rounded-full animate-spin" />
                : <Trash2 className="w-3.5 h-3.5" />}
            </button>
          </div>
        ))}
        {tokens.length === 0 && !loadError && (
          <p className="text-gray-400 text-sm text-center py-4">No tokens yet.</p>
        )}
      </div>

      {showCreateForm ? (
        <div className="space-y-2">
          <input
            autoFocus
            type="text"
            value={newName}
            onChange={e => setNewName(e.target.value)}
            onKeyDown={e => { if (e.key === 'Escape') setShowCreateForm(false) }}
            placeholder="Token name (e.g. Terraform CI)"
            className="w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
          />
          <div className="flex gap-2">
            <select
              value={newScope}
              onChange={e => setNewScope(e.target.value as TokenScope)}
              className="flex-1 bg-white border border-gray-100 rounded-xl px-3 py-2.5 text-sm text-gray-700 focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
            >
              <option value="read_only">Read-only</option>
              <option value="read_write">Read-write</option>
            </select>
            <select
              value={newExpiresInDays ?? ''}
              onChange={e => setNewExpiresInDays(e.target.value === '' ? null : Number(e.target.value))}
              className="flex-1 bg-white border border-gray-100 rounded-xl px-3 py-2.5 text-sm text-gray-700 focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
            >
              <option value="30">Expires in 30 days</option>
              <option value="90">Expires in 90 days</option>
              <option value="365">Expires in 1 year</option>
              <option value="">Never expires</option>
            </select>
          </div>
          <div className="flex gap-2">
            <button
              onClick={() => void handleCreate()}
              disabled={creating || !newName.trim()}
              className="px-6 py-2.5 bg-brand-600 hover:bg-brand-700 disabled:opacity-50 text-white text-sm font-bold rounded-xl transition-all shadow-lg shadow-brand-200"
            >
              {creating ? '…' : 'Create'}
            </button>
            <button
              onClick={() => { setShowCreateForm(false); setNewName('') }}
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
          <Plus className="w-3.5 h-3.5" /> Generate new token
        </button>
      )}
    </SectionCard>
  )
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

export default function Settings() {
  const { logout, session } = useAuth()
  const { activeProject } = useProject()
  const navigate = useNavigate()

  async function handleLogout() {
    await logout()
    navigate('/login')
  }

  const apiOrigin = import.meta.env.VITE_API_URL || window.location.origin

  const jsSnippet = `// npm install @checkgate/web
import { createClient } from '@checkgate/web'

const client = createClient({
  sdkKey: 'sk_live_…',
  serverUrl: '${apiOrigin}',
})

await client.connect()

const darkMode = client.getBool('dark_mode', false)
console.log('dark_mode →', darkMode)`

  const nodeSnippet = `// npm install @checkgate/node
import Checkgate from '@checkgate/node'

const client = new Checkgate({
  sdkKey: process.env.CHECKGATE_SDK_KEY,
  serverUrl: '${apiOrigin}',
})

await client.connect()

const enabled = client.getBool('feature_x', false, { userId: req.user.id })`

  return (
    <div className="w-full space-y-5">
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-5 items-start">
        {/* Main column — things you actively use/copy */}
        <div className="lg:col-span-2 space-y-5">
          {/* SDK quick-start */}
          <SectionCard
            icon={Code2}
            title="SDK quick-start"
            description="Copy a snippet to connect your app to this Checkgate instance."
          >
            <div className="space-y-4">
              <div>
                <p className="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">
                  Browser / Node (ESM)
                </p>
                <CodeBlock code={jsSnippet} />
              </div>
              <div>
                <p className="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">
                  Node.js (server-side evaluation)
                </p>
                <CodeBlock code={nodeSnippet} />
              </div>
              <p className="text-xs text-gray-400">
                Replace <code className="text-gray-600">sk_live_…</code> with an SDK key from the{' '}
                {activeProject ? (
                  <Link to={`/projects/${activeProject.id}?tab=keys`} className="text-brand-600 hover:underline">
                    Project Settings → SDK Keys
                  </Link>
                ) : (
                  <span className="text-gray-500">Project Settings → SDK Keys</span>
                )}{' '}
                page (or the key shown during initial setup).
              </p>
            </div>
          </SectionCard>

          {/* Personal access tokens */}
          <PersonalAccessTokensSection />
        </div>

        {/* Sidebar — informational, rarely touched */}
        <div className="space-y-5">
          {/* Auth info */}
          <SectionCard
            icon={Shield}
            title="Authentication"
            description="Sessions use HttpOnly encrypted cookies — the SDK key is never exposed to JavaScript."
          >
            <div className="space-y-3">
              <div className="flex items-start gap-3 p-3 rounded-lg bg-brand-50 border border-brand-200">
                <Info className="w-4 h-4 text-brand-600 mt-0.5 shrink-0" />
                <p className="text-xs text-gray-600 leading-relaxed">
                  Your SDK key is validated server-side at login. After that, a short-lived
                  encrypted cookie keeps you authenticated. The key is never stored in your browser.
                </p>
              </div>
              <div className="grid grid-cols-2 gap-3 text-xs">
                <div className="p-3 rounded-lg bg-gray-50 border border-gray-200">
                  <p className="text-gray-500 mb-0.5">Cookie flags</p>
                  <p className="text-gray-800 font-mono">HttpOnly · SameSite=Strict</p>
                </div>
                <div className="p-3 rounded-lg bg-gray-50 border border-gray-200">
                  <p className="text-gray-500 mb-0.5">Session TTL</p>
                  <p className="text-gray-800 font-mono">7 days</p>
                </div>
              </div>
            </div>
          </SectionCard>

          {/* API endpoint */}
          <SectionCard
            icon={Globe}
            title="API endpoint"
            description="The dashboard communicates with the Checkgate server at this origin."
          >
            <div className="flex items-center gap-2 p-3 rounded-lg bg-gray-50 border border-gray-200">
              <code className="text-brand-600 text-sm flex-1 truncate">{apiOrigin}</code>
            </div>
            <p className="mt-2 text-xs text-gray-400">
              Set <code className="text-gray-600">VITE_API_URL</code> at build time to point at a different server.
            </p>
          </SectionCard>

          {/* Account */}
          <SectionCard
            icon={LogOut}
            title="Account"
            description={`Signed in as ${session?.user.email ?? '—'} · ${session?.user.role ?? ''}`}
          >
            <button
              onClick={() => void handleLogout()}
              className="flex items-center gap-2 px-4 py-2 bg-gray-100 hover:bg-gray-200 text-gray-700 text-sm font-medium rounded-lg transition-colors border border-gray-200"
            >
              <LogOut className="w-3.5 h-3.5" /> Sign out
            </button>
          </SectionCard>
        </div>
      </div>
    </div>
  )
}
