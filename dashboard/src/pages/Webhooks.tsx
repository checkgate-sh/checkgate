import { useEffect, useState } from 'react'
import { Webhook as WebhookIcon, Plus, Pencil, Trash2, ChevronRight } from 'lucide-react'
import { webhooksApi } from '../api'
import { useEnvironment } from '../context/EnvironmentContext'
import type { Webhook, WebhookDelivery } from '../types'

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function statusBadge(code: number | null, error: string | null) {
  if (error && !code) {
    return (
      <span className="px-2 py-0.5 rounded text-xs font-medium bg-red-100 text-red-700">
        error
      </span>
    )
  }
  if (code && code >= 200 && code < 300) {
    return (
      <span className="px-2 py-0.5 rounded text-xs font-medium bg-brand-100 text-brand-700">
        {code}
      </span>
    )
  }
  return (
    <span className="px-2 py-0.5 rounded text-xs font-medium bg-amber-100 text-amber-700">
      {code ?? '—'}
    </span>
  )
}

// ---------------------------------------------------------------------------
// Delivery log panel
// ---------------------------------------------------------------------------

function DeliveryLog({
  webhook,
  onClose,
}: {
  webhook: Webhook
  onClose: () => void
}) {
  const { activeEnv } = useEnvironment()
  const [rows, setRows] = useState<WebhookDelivery[]>([])
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    if (!activeEnv) return
    setLoading(true)
    webhooksApi
      .listDeliveries(activeEnv.id, webhook.id)
      .then(setRows)
      .finally(() => setLoading(false))
  }, [activeEnv, webhook.id])

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40">
      <div className="bg-white rounded-xl shadow-xl w-full max-w-3xl max-h-[80vh] flex flex-col">
        <div className="flex items-center justify-between px-6 py-4 border-b">
          <div>
            <h2 className="font-semibold text-gray-900">Delivery log</h2>
            <p className="text-sm text-gray-500">{webhook.name}</p>
          </div>
          <button onClick={onClose} className="text-gray-400 hover:text-gray-600 text-xl leading-none">
            ✕
          </button>
        </div>

        <div className="overflow-y-auto flex-1">
          {loading ? (
            <p className="p-6 text-gray-500 text-sm">Loading…</p>
          ) : rows.length === 0 ? (
            <p className="p-6 text-gray-500 text-sm">No deliveries yet.</p>
          ) : (
            <table className="w-full text-sm">
              <thead className="bg-gray-50 text-gray-500 text-xs uppercase sticky top-0">
                <tr>
                  <th className="px-4 py-2 text-left">Time</th>
                  <th className="px-4 py-2 text-left">Event</th>
                  <th className="px-4 py-2 text-left">Status</th>
                  <th className="px-4 py-2 text-left">Response / Error</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-100">
                {rows.map((d) => (
                  <tr key={d.id} className="hover:bg-gray-50">
                    <td className="px-4 py-2 whitespace-nowrap text-gray-500">
                      {new Date(Number(d.delivered_at) * 1000).toLocaleString()}
                    </td>
                    <td className="px-4 py-2 font-mono text-gray-700">{d.event}</td>
                    <td className="px-4 py-2">{statusBadge(d.status_code, d.error)}</td>
                    <td className="px-4 py-2 text-gray-500 truncate max-w-xs">
                      {d.error ?? d.response_body ?? '—'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Create / edit modal
// ---------------------------------------------------------------------------

interface FormState {
  name: string
  url: string
  secret: string
  enabled: boolean
}

function WebhookModal({
  initial,
  onSave,
  onCancel,
}: {
  initial?: Webhook
  onSave: (f: FormState) => Promise<void>
  onCancel: () => void
}) {
  const [form, setForm] = useState<FormState>({
    name: initial?.name ?? '',
    url: initial?.url ?? '',
    secret: '',
    enabled: initial?.enabled ?? true,
  })
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setSaving(true)
    setError(null)
    try {
      await onSave(form)
    } catch (err) {
      setError(String(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40">
      <form
        onSubmit={submit}
        className="bg-white rounded-xl shadow-xl w-full max-w-md p-6 space-y-4"
      >
        <h2 className="font-semibold text-gray-900 text-lg">
          {initial ? 'Edit webhook' : 'New webhook'}
        </h2>

        {error && <p className="text-red-600 text-sm">{error}</p>}

        <label className="block">
          <span className="text-sm font-medium text-gray-700">Name</span>
          <input
            className="mt-1 w-full border border-gray-300 rounded-lg px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-indigo-500"
            value={form.name}
            onChange={(e) =>
              setForm((f) => ({ ...f, name: (e.target as HTMLInputElement).value }))
            }
            required
          />
        </label>

        <label className="block">
          <span className="text-sm font-medium text-gray-700">URL</span>
          <input
            type="url"
            className="mt-1 w-full border border-gray-300 rounded-lg px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-indigo-500"
            value={form.url}
            onChange={(e) =>
              setForm((f) => ({ ...f, url: (e.target as HTMLInputElement).value }))
            }
            placeholder="https://example.com/hook"
            required
          />
        </label>

        <label className="block">
          <span className="text-sm font-medium text-gray-700">
            Secret{initial?.has_secret ? ' (leave blank to keep existing)' : ' (optional)'}
          </span>
          <input
            className="mt-1 w-full border border-gray-300 rounded-lg px-3 py-2 text-sm font-mono focus:outline-none focus:ring-2 focus:ring-indigo-500"
            value={form.secret}
            onChange={(e) =>
              setForm((f) => ({ ...f, secret: (e.target as HTMLInputElement).value }))
            }
            placeholder={initial?.has_secret ? '••••••••' : ''}
          />
          <p className="text-xs text-gray-400 mt-1">
            Used to sign payloads with HMAC-SHA256 (<code>X-Checkgate-Signature</code>).
          </p>
        </label>

        <label className="flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            className="w-4 h-4 rounded text-indigo-600"
            checked={form.enabled}
            onChange={(e) =>
              setForm((f) => ({ ...f, enabled: (e.target as HTMLInputElement).checked }))
            }
          />
          <span className="text-sm text-gray-700">Enabled</span>
        </label>

        <div className="flex justify-end gap-3 pt-2">
          <button
            type="button"
            onClick={onCancel}
            className="px-4 py-2 text-sm rounded-lg border border-gray-300 hover:bg-gray-50"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={saving}
            className="px-4 py-2 text-sm rounded-lg bg-indigo-600 text-white hover:bg-indigo-700 disabled:opacity-50"
          >
            {saving ? 'Saving…' : 'Save'}
          </button>
        </div>
      </form>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

export default function Webhooks() {
  const { activeEnv } = useEnvironment()
  const [hooks, setHooks] = useState<Webhook[]>([])
  const [loading, setLoading] = useState(true)
  const [modal, setModal] = useState<'create' | Webhook | null>(null)
  const [deliveryWebhook, setDeliveryWebhook] = useState<Webhook | null>(null)
  const [deleting, setDeleting] = useState<string | null>(null)

  async function load() {
    if (!activeEnv) return
    setLoading(true)
    try {
      setHooks(await webhooksApi.list(activeEnv.id))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    load()
  }, [activeEnv?.id])

  async function handleSave(form: FormState) {
    if (!activeEnv) return
    if (modal === 'create') {
      const created = await webhooksApi.create(activeEnv.id, {
        name: form.name,
        url: form.url,
        secret: form.secret || undefined,
        enabled: form.enabled,
      })
      setHooks((h) => [...h, created])
    } else if (modal && typeof modal === 'object') {
      const patch: Record<string, unknown> = {
        name: form.name,
        url: form.url,
        enabled: form.enabled,
      }
      if (form.secret) patch.secret = form.secret
      const updated = await webhooksApi.patch(activeEnv.id, modal.id, patch)
      setHooks((h) => h.map((w) => (w.id === updated.id ? updated : w)))
    }
    setModal(null)
  }

  async function handleDelete(hook: Webhook) {
    if (!activeEnv || !confirm(`Delete webhook "${hook.name}"?`)) return
    setDeleting(hook.id)
    try {
      await webhooksApi.delete(activeEnv.id, hook.id)
      setHooks((h) => h.filter((w) => w.id !== hook.id))
    } finally {
      setDeleting(null)
    }
  }

  async function toggleEnabled(hook: Webhook) {
    if (!activeEnv) return
    const updated = await webhooksApi.patch(activeEnv.id, hook.id, { enabled: !hook.enabled })
    setHooks((h) => h.map((w) => (w.id === updated.id ? updated : w)))
  }

  return (
    <div className="w-full space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-gray-900">Webhooks</h1>
          <p className="text-gray-500 text-sm mt-1">
            Notify external systems when flags change in{' '}
            <span className="font-medium">{activeEnv?.name ?? 'this environment'}</span>.
          </p>
        </div>
        <button
          onClick={() => setModal('create')}
          className="flex items-center gap-2 px-4 py-2 text-sm rounded-lg bg-indigo-600 text-white hover:bg-indigo-700"
        >
          <Plus className="w-4 h-4" />
          New webhook
        </button>
      </div>

      {loading ? (
        <p className="text-gray-500">Loading…</p>
      ) : hooks.length === 0 ? (
        <div className="border border-dashed border-gray-300 rounded-xl p-12 text-center text-gray-400">
          <WebhookIcon className="w-8 h-8 mx-auto mb-3 opacity-40" />
          <p>No webhooks yet. Click "New webhook" to add one.</p>
        </div>
      ) : (
        <div className="space-y-3">
          {hooks.map((hook) => (
            <div
              key={hook.id}
              className="flex items-center gap-4 bg-white border border-gray-200 rounded-xl px-5 py-4"
            >
              <WebhookIcon className="w-4 h-4 text-gray-400 shrink-0" />
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="font-medium text-gray-900">{hook.name}</span>
                  {hook.enabled ? (
                    <span className="px-2 py-0.5 rounded text-xs bg-brand-100 text-brand-700">
                      active
                    </span>
                  ) : (
                    <span className="px-2 py-0.5 rounded text-xs bg-gray-100 text-gray-500">
                      disabled
                    </span>
                  )}
                  {hook.has_secret && (
                    <span className="px-2 py-0.5 rounded text-xs bg-indigo-50 text-indigo-600">
                      signed
                    </span>
                  )}
                </div>
                <p className="text-sm text-gray-500 font-mono truncate">{hook.url}</p>
              </div>

              <div className="flex items-center gap-3 shrink-0 text-sm">
                <button
                  onClick={() => setDeliveryWebhook(hook)}
                  className="flex items-center gap-1 text-gray-500 hover:text-gray-700"
                >
                  <ChevronRight className="w-3.5 h-3.5" />
                  Deliveries
                </button>
                <button
                  onClick={() => toggleEnabled(hook)}
                  className="text-gray-500 hover:text-gray-700"
                >
                  {hook.enabled ? 'Disable' : 'Enable'}
                </button>
                <button
                  onClick={() => setModal(hook)}
                  className="text-indigo-600 hover:text-indigo-800"
                >
                  <Pencil className="w-4 h-4" />
                </button>
                <button
                  onClick={() => handleDelete(hook)}
                  disabled={deleting === hook.id}
                  className="text-red-400 hover:text-red-600 disabled:opacity-40"
                >
                  <Trash2 className="w-4 h-4" />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {modal !== null && (
        <WebhookModal
          initial={modal === 'create' ? undefined : modal}
          onSave={handleSave}
          onCancel={() => setModal(null)}
        />
      )}

      {deliveryWebhook && (
        <DeliveryLog webhook={deliveryWebhook} onClose={() => setDeliveryWebhook(null)} />
      )}
    </div>
  )
}
