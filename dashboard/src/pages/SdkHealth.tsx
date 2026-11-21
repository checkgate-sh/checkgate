import { useEffect, useState } from 'react'
import { Wifi } from 'lucide-react'
import { healthApi } from '../api'
import type { ConnectedClient, EnvironmentHealth } from '../types'
import { useEnvironment } from '../context/EnvironmentContext'

function relativeTime(unixSecs: number): string {
  const diff = Math.floor(Date.now() / 1000) - unixSecs
  if (diff < 60) return `${diff}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  return `${Math.floor(diff / 3600)}h ago`
}

export default function SdkHealth() {
  const [clients, setClients] = useState<ConnectedClient[]>([])
  const [envHealth, setEnvHealth] = useState<EnvironmentHealth[]>([])
  const { environments } = useEnvironment()
  const envName = (id: string | null) =>
    (id && environments.find((e) => e.id === id)?.name) || (id ? id.slice(0, 8) : '—')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  // Tick to re-compute relative timestamps without re-fetching.
  const [, setTick] = useState(0)

  async function load() {
    try {
      const [data, envs] = await Promise.all([healthApi.connections(), healthApi.environments()])
      setClients(data)
      setEnvHealth(envs)
      setError(null)
    } catch (err) {
      setError(String(err))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    load()
    const refreshInterval = setInterval(() => load(), 10_000)
    const tickInterval = setInterval(() => setTick((t) => t + 1), 1_000)
    return () => {
      clearInterval(refreshInterval)
      clearInterval(tickInterval)
    }
  }, [])

  return (
    <div className="w-full space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-gray-900">SDK Health</h1>
          <p className="text-gray-500 text-sm mt-1">
            Live SSE connections to this server instance, for the projects you can access.
            Refreshes every 10 s.
          </p>
          <p className="text-gray-400 text-xs mt-1">
            Each instance tracks its own connections. Client addresses are shown as a network
            prefix unless you are a workspace admin.
          </p>
          <p className="text-gray-400 text-xs mt-1">
            This is what the <strong>server observes</strong>: which streams are open and which
            revision each has been sent. It does not prove an SDK applied it, and SDKs that poll or
            serve their offline cache do not appear here. For an SDK's own state use{' '}
            <code className="font-mono">getHealth()</code> in the application.
          </p>
        </div>
        <button
          onClick={() => { setLoading(true); load() }}
          className="px-3 py-1.5 text-sm rounded-lg border border-gray-300 hover:bg-gray-50"
        >
          Refresh
        </button>
      </div>

      {error && (
        <div className="bg-red-50 border border-red-200 text-red-700 text-sm px-4 py-3 rounded-lg">
          {error}
        </div>
      )}

      {envHealth.length > 0 && (
        <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3" data-testid="env-health">
          {envHealth.map((e) => (
            <div key={e.environment_id} className="bg-white border border-gray-200 rounded-xl px-5 py-4">
              <div className="flex items-center justify-between">
                <span className="text-sm font-semibold text-gray-900">{envName(e.environment_id)}</span>
                <span
                  className={`text-xs font-bold px-2 py-0.5 rounded ${
                    e.connections_behind === 0 ? 'bg-emerald-50 text-emerald-700' : 'bg-amber-50 text-amber-700'
                  }`}
                >
                  {e.connections_behind === 0 ? 'all streams current' : `${e.connections_behind} behind`}
                </span>
              </div>
              <p className="text-xs text-gray-500 mt-1">
                {e.connections} stream{e.connections !== 1 ? 's' : ''} · latest revision{' '}
                <span className="font-mono">{e.latest_revision ?? '—'}</span>
                {e.oldest_connected_at !== null && <> · oldest {relativeTime(e.oldest_connected_at)}</>}
              </p>
            </div>
          ))}
        </div>
      )}

      <div className="bg-white border border-gray-200 rounded-xl overflow-hidden">
        <div className="px-5 py-3 border-b border-gray-100 flex items-center gap-2">
          <Wifi className="w-4 h-4 text-gray-400" />
          <span className="text-sm font-medium text-gray-700">
            {loading ? '…' : clients.length} connection{clients.length !== 1 ? 's' : ''}
          </span>
          {clients.length > 0 && (
            <span className="w-2 h-2 rounded-full bg-brand-400 animate-pulse inline-block ml-1" />
          )}
        </div>

        {loading ? (
          <p className="px-5 py-8 text-gray-400 text-sm">Loading…</p>
        ) : clients.length === 0 ? (
          <p className="px-5 py-8 text-gray-400 text-sm">
            No active SSE connections on this instance.
          </p>
        ) : (
          <table className="w-full text-sm">
            <thead className="bg-gray-50 text-gray-500 text-xs uppercase">
              <tr>
                <th className="px-5 py-2 text-left">Connection</th>
                <th className="px-5 py-2 text-left">SDK Key</th>
                <th className="px-5 py-2 text-left">Environment</th>
                <th className="px-5 py-2 text-left">Client IP</th>
                <th className="px-5 py-2 text-left">Connected</th>
                <th className="px-5 py-2 text-left" title="Highest revision written to this stream">
                  Delivered rev.
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-100">
              {clients.map((c) => (
                <tr key={c.connection_id} className="hover:bg-gray-50">
                  <td className="px-5 py-3 font-mono text-gray-400 text-xs">
                    {c.connection_id}
                  </td>
                  <td className="px-5 py-3 text-gray-700">
                    {c.sdk_key_name ?? (
                      <span className="text-gray-400 italic">dashboard</span>
                    )}
                  </td>
                  <td className="px-5 py-3 text-gray-500 text-xs">{envName(c.environment_id)}</td>
                  <td className="px-5 py-3 text-gray-500">{c.client_ip}</td>
                  <td className="px-5 py-3 text-gray-400">{relativeTime(c.connected_at)}</td>
                  <td className="px-5 py-3 text-gray-500 font-mono text-xs">
                    {c.last_delivered_revision ?? '—'}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  )
}
