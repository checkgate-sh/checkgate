import { useCallback, useEffect, useState } from 'react'
import { Plus, Send, Trash2, RotateCcw, Check, MessageSquare } from 'lucide-react'
import { integrationsApi } from '../api'
import { INTEGRATION_EVENTS, type Integration, type IntegrationKind } from '../types'
import { useEnvironment } from '../context/EnvironmentContext'

const KIND_LABEL: Record<IntegrationKind, string> = {
  slack: 'Slack',
  teams: 'Microsoft Teams',
}

const KIND_HINT: Record<IntegrationKind, string> = {
  slack: 'Slack → Apps → Incoming Webhooks → Add to Workspace, then copy the hook URL.',
  teams: 'Teams channel → Connectors → Incoming Webhook → Create, then copy the URL.',
}

const inputClass =
  'w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium'

/** Friendly label for an event name, e.g. `flag.created` → "Flag created". */
function eventLabel(event: string): string {
  const [group, action] = event.split('.')
  const subject = group === 'change_request' ? 'Change request' : 'Flag'
  return `${subject} ${action}`
}

function CreateForm({ envId, onCreated }: { envId: string; onCreated: (i: Integration) => void }) {
  const [kind, setKind] = useState<IntegrationKind>('slack')
  const [name, setName] = useState('')
  const [url, setUrl] = useState('')
  const [events, setEvents] = useState<string[]>([])
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  function toggleEvent(event: string) {
    setEvents(prev => (prev.includes(event) ? prev.filter(e => e !== event) : [...prev, event]))
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setSaving(true)
    setError(null)
    try {
      const created = await integrationsApi.create(envId, {
        kind,
        name,
        webhook_url: url,
        events,
      })
      onCreated(created)
      setName('')
      setUrl('')
      setEvents([])
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create integration')
    } finally {
      setSaving(false)
    }
  }

  return (
    <form onSubmit={e => void submit(e)} className="premium-card shadow-premium-lg border-none p-6 space-y-4">
      <h2 className="font-display font-bold text-sm tracking-tight text-gray-900">Connect a channel</h2>

      {error && (
        <div className="p-3 rounded-lg bg-red-50 border border-red-200 text-red-600 text-sm">{error}</div>
      )}

      <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1.5">Provider</label>
          <div className="flex gap-2">
            {(Object.keys(KIND_LABEL) as IntegrationKind[]).map(k => (
              <button
                key={k}
                type="button"
                onClick={() => setKind(k)}
                className={`flex-1 px-4 py-2.5 rounded-xl text-sm font-semibold border transition-all ${
                  kind === k
                    ? 'bg-brand-50 border-brand-200 text-brand-700'
                    : 'bg-white border-gray-100 text-gray-500 hover:text-gray-700'
                }`}
              >
                {KIND_LABEL[k]}
              </button>
            ))}
          </div>
        </div>

        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1.5">Name</label>
          <input
            required
            value={name}
            onChange={e => setName(e.target.value)}
            placeholder="e.g. #engineering"
            className={inputClass}
          />
        </div>
      </div>

      <div>
        <label className="block text-sm font-medium text-gray-700 mb-1.5">Incoming webhook URL</label>
        <input
          required
          type="url"
          value={url}
          onChange={e => setUrl(e.target.value)}
          placeholder="https://hooks.slack.com/services/…"
          className={`${inputClass} font-mono text-xs`}
        />
        <p className="mt-1.5 text-xs text-gray-400">{KIND_HINT[kind]}</p>
        <p className="mt-1 text-xs text-gray-400">
          Stored write-only — anyone with this URL can post to the channel, so it is never shown again.
        </p>
      </div>

      <div>
        <label className="block text-sm font-medium text-gray-700 mb-1.5">Events</label>
        <div className="flex flex-wrap gap-1.5">
          {INTEGRATION_EVENTS.map(event => {
            const on = events.includes(event)
            return (
              <button
                key={event}
                type="button"
                onClick={() => toggleEvent(event)}
                className={`px-2.5 py-1 rounded-lg text-xs font-medium border transition-all ${
                  on
                    ? 'bg-brand-50 border-brand-200 text-brand-700'
                    : 'bg-white border-gray-100 text-gray-400 hover:text-gray-600'
                }`}
              >
                {eventLabel(event)}
              </button>
            )
          })}
        </div>
        <p className="mt-1.5 text-xs text-gray-400">
          {events.length === 0
            ? 'Nothing selected — every event will be sent.'
            : `${events.length} selected.`}
        </p>
      </div>

      <button
        type="submit"
        disabled={saving}
        className="flex items-center gap-2 px-5 py-2.5 bg-brand-600 hover:bg-brand-700 disabled:opacity-60 text-white text-sm font-bold rounded-xl transition-all shadow-lg shadow-brand-200"
      >
        <Plus className="w-4 h-4" /> {saving ? 'Connecting…' : 'Connect'}
      </button>
    </form>
  )
}

function IntegrationRow({
  integration,
  envId,
  onChanged,
  onDeleted,
}: {
  integration: Integration
  envId: string
  onChanged: (i: Integration) => void
  onDeleted: (id: string) => void
}) {
  const [busy, setBusy] = useState(false)
  const [tested, setTested] = useState(false)

  async function toggleEnabled() {
    setBusy(true)
    try {
      onChanged(await integrationsApi.patch(envId, integration.id, { enabled: !integration.enabled }))
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Update failed')
    } finally {
      setBusy(false)
    }
  }

  async function sendTest() {
    setBusy(true)
    try {
      await integrationsApi.test(envId, integration.id)
      setTested(true)
      setTimeout(() => setTested(false), 2500)
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Test failed')
    } finally {
      setBusy(false)
    }
  }

  async function remove() {
    if (!confirm(`Disconnect "${integration.name}"? Events will stop being sent to this channel.`)) return
    setBusy(true)
    try {
      await integrationsApi.delete(envId, integration.id)
      onDeleted(integration.id)
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Delete failed')
      setBusy(false)
    }
  }

  return (
    <tr className={`group hover:bg-brand-50/20 transition-all ${integration.enabled ? '' : 'opacity-50'}`}>
      <td className="px-8 py-5">
        <span className="font-semibold text-gray-900 text-sm">{integration.name}</span>
        <div className="text-xs text-gray-400 mt-0.5">
          {KIND_LABEL[integration.kind]} · <span className="font-mono">{integration.webhook_url_preview}</span>
        </div>
      </td>
      <td className="px-8 py-5">
        {integration.events.length === 0 ? (
          <span className="text-xs text-gray-400">All events</span>
        ) : (
          <div className="flex flex-wrap gap-1">
            {integration.events.map(e => (
              <span key={e} className="px-1.5 py-0.5 bg-gray-50 text-gray-500 rounded text-[9px] font-medium">
                {eventLabel(e)}
              </span>
            ))}
          </div>
        )}
      </td>
      <td className="px-8 py-5">
        <button
          onClick={() => void toggleEnabled()}
          disabled={busy}
          className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
            integration.enabled ? 'bg-brand-600' : 'bg-gray-200'
          }`}
          aria-label={integration.enabled ? 'Disable integration' : 'Enable integration'}
        >
          <span
            className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition-transform ${
              integration.enabled ? 'translate-x-4' : 'translate-x-0.5'
            }`}
          />
        </button>
      </td>
      <td className="px-8 py-5">
        <div className="flex items-center gap-1 justify-end opacity-0 group-hover:opacity-100 transition-all">
          <button
            onClick={() => void sendTest()}
            disabled={busy}
            className="p-1.5 rounded-md text-gray-400 hover:text-brand-600 hover:bg-brand-50 transition-colors"
            title="Send a test message"
            aria-label="Send test message"
          >
            {tested ? <Check className="w-3.5 h-3.5 text-brand-500" /> : <Send className="w-3.5 h-3.5" />}
          </button>
          <button
            onClick={() => void remove()}
            disabled={busy}
            className="p-1.5 rounded-md text-gray-400 hover:text-red-500 hover:bg-red-50 transition-colors"
            aria-label="Disconnect integration"
          >
            <Trash2 className="w-3.5 h-3.5" />
          </button>
        </div>
      </td>
    </tr>
  )
}

export default function Integrations() {
  const { activeEnv } = useEnvironment()
  const [integrations, setIntegrations] = useState<Integration[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    if (!activeEnv) return
    try {
      setError(null)
      setIntegrations(await integrationsApi.list(activeEnv.id))
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load integrations')
    } finally {
      setLoading(false)
    }
  }, [activeEnv])

  useEffect(() => {
    setLoading(true)
    void load()
  }, [load])

  if (!activeEnv) {
    return <div className="text-sm text-gray-400">Select an environment first.</div>
  }

  return (
    <div className="w-full space-y-4">
      <p className="text-sm text-gray-500">
        Send flag and change-request activity to Slack or Microsoft Teams. Configured per environment —
        these apply to <span className="font-semibold text-gray-700">{activeEnv.name}</span>.
      </p>

      <CreateForm envId={activeEnv.id} onCreated={i => setIntegrations(prev => [...prev, i])} />

      <div className="premium-card shadow-premium-lg border-none bg-white">
        {loading ? (
          <div className="flex items-center justify-center h-32 text-gray-400 text-sm">Loading…</div>
        ) : error ? (
          <div className="flex flex-col items-center justify-center h-32 gap-3">
            <p className="text-red-500 text-sm">{error}</p>
            <button
              onClick={() => { setLoading(true); void load() }}
              className="flex items-center gap-1.5 text-sm text-gray-500 hover:text-gray-800"
            >
              <RotateCcw className="w-3.5 h-3.5" /> Retry
            </button>
          </div>
        ) : integrations.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-32 gap-2 text-gray-400">
            <MessageSquare className="w-5 h-5" />
            <p className="text-sm">No channels connected yet.</p>
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-50 bg-gray-50/50">
                  <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Channel</th>
                  <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Events</th>
                  <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Enabled</th>
                  <th className="px-8 py-4" />
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-50/50">
                {integrations.map(i => (
                  <IntegrationRow
                    key={i.id}
                    integration={i}
                    envId={activeEnv.id}
                    onChanged={updated =>
                      setIntegrations(prev => prev.map(x => (x.id === updated.id ? updated : x)))
                    }
                    onDeleted={id => setIntegrations(prev => prev.filter(x => x.id !== id))}
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  )
}

export { eventLabel }
