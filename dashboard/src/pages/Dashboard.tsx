import { useEffect, useState, useCallback } from 'react'
import { Link } from 'react-router-dom'
import { ToggleLeft, ToggleRight, ListFilter, CalendarClock, Plus, ArrowRight } from 'lucide-react'
import { api, scheduledApi } from '../api'
import type { Flag, ScheduledChange } from '../types'
import { useEnvironment } from '../context/EnvironmentContext'

interface StatCardProps {
  label: string
  value: number | string
  icon: React.ElementType
  iconBg: string
  iconColor: string
}

function StatCard({ label, value, icon: Icon, iconBg, iconColor }: StatCardProps) {
  return (
    <div className="premium-card p-6 flex items-start gap-4 hover:shadow-premium-lg transition-shadow">
      <div className={`w-12 h-12 rounded-2xl flex items-center justify-center shrink-0 ${iconBg} bg-opacity-30`}>
        <Icon className={`w-6 h-6 ${iconColor}`} />
      </div>
      <div>
        <p className="text-gray-500 text-xs font-semibold uppercase tracking-wider">{label}</p>
        <p className="text-3xl font-display font-bold text-gray-900 mt-1">{value}</p>
      </div>
    </div>
  )
}

export default function Dashboard() {
  const { activeEnv } = useEnvironment()
  const [flags, setFlags] = useState<Flag[]>([])
  const [pendingChanges, setPendingChanges] = useState<ScheduledChange[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    if (!activeEnv) return
    try {
      setError(null)
      const [flagData, schedData] = await Promise.all([
        api.listFlags(activeEnv.id),
        scheduledApi.list(activeEnv.id).catch(() => [] as ScheduledChange[]),
      ])
      setFlags(flagData)
      setPendingChanges(schedData.filter(c => !c.executed_at))
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load flags')
    } finally {
      setLoading(false)
    }
  }, [activeEnv])

  useEffect(() => { void load() }, [load])

  const enabled = flags.filter(f => f.is_enabled).length
  const withRules = flags.filter(f => f.rules.length > 0).length

  return (
    <div className="w-full space-y-6">
      {/* Stats */}
      <div className="grid grid-cols-2 xl:grid-cols-4 gap-4">
        <StatCard label="Total flags" value={loading ? '—' : flags.length} icon={ToggleLeft} iconBg="bg-brand-50" iconColor="text-brand-600" />
        <StatCard label="Enabled" value={loading ? '—' : enabled} icon={ToggleRight} iconBg="bg-brand-50" iconColor="text-brand-600" />
        <StatCard label="With rules" value={loading ? '—' : withRules} icon={ListFilter} iconBg="bg-amber-50" iconColor="text-amber-600" />
        <Link to="/schedule" className="block hover:no-underline">
          <StatCard
            label="Scheduled changes"
            value={loading ? '—' : pendingChanges.length}
            icon={CalendarClock}
            iconBg="bg-indigo-50"
            iconColor="text-indigo-600"
          />
        </Link>
      </div>

      {/* Flags table */}
      <div className="premium-card border-none shadow-premium-lg">
        <div className="flex items-center justify-between px-8 py-5 border-b border-gray-50 bg-white">
          <h2 className="text-gray-900 font-display font-bold text-base">Recent flags</h2>
          <Link
            to="/flags"
            className="flex items-center gap-1.5 text-brand-600 hover:text-brand-700 text-sm font-bold transition-colors"
          >
            View all <ArrowRight className="w-4 h-4" />
          </Link>
        </div>

        {loading ? (
          <div className="flex items-center justify-center h-32 text-gray-400 text-sm">Loading…</div>
        ) : error ? (
          <div className="flex items-center justify-center h-32 text-red-500 text-sm">{error}</div>
        ) : flags.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-32 gap-3">
            <p className="text-gray-400 text-sm">No flags yet.</p>
            <Link
              to="/flags?new=1"
              className="flex items-center gap-1.5 text-sm text-brand-600 hover:text-brand-700 font-medium transition-colors"
            >
              <Plus className="w-3.5 h-3.5" /> Create your first flag
            </Link>
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                  <tr className="border-b border-gray-50/50 bg-gray-50/30">
                    <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Key</th>
                    <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest hidden md:table-cell">Description</th>
                    <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Rollout</th>
                    <th className="text-left px-8 py-4 text-gray-400 font-bold text-[10px] uppercase tracking-widest">Status</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-gray-50/50">
                  {flags.slice(0, 8).map(flag => (
                    <tr key={flag.key} className="hover:bg-brand-50/20 transition-all">
                      <td className="px-8 py-5">
                      <Link
                        to={`/flags?edit=${encodeURIComponent(flag.key)}`}
                        className="font-mono text-brand-600 hover:text-brand-700 transition-colors text-sm font-semibold"
                      >
                        {flag.key}
                      </Link>
                    </td>
                      <td className="px-8 py-5 text-gray-500 text-xs max-w-xs truncate hidden md:table-cell">
                        {flag.description ?? <span className="text-gray-300 italic">No description</span>}
                      </td>
                      <td className="px-8 py-5 text-gray-900 font-medium font-mono text-xs">
                        {flag.rollout_percentage != null ? `${flag.rollout_percentage}%` : '100%'}
                      </td>
                      <td className="px-8 py-5">
                        <span className={`inline-flex items-center gap-1.5 px-3 py-1 rounded-full text-[10px] font-bold uppercase tracking-wide ${
                          flag.is_enabled
                            ? 'bg-brand-50 text-brand-700 ring-1 ring-brand-200'
                            : 'bg-gray-100 text-gray-500 ring-1 ring-gray-200'
                        }`}>
                          <span className={`w-2 h-2 rounded-full ${flag.is_enabled ? 'bg-brand-500' : 'bg-gray-400'} shadow-sm`} />
                          {flag.is_enabled ? 'Active' : 'Paused'}
                        </span>
                      </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {/* Upcoming scheduled changes */}
      {!loading && pendingChanges.length > 0 && (
        <div className="premium-card border-none shadow-premium-lg">
          <div className="flex items-center justify-between px-8 py-5 border-b border-gray-50 bg-white">
            <h2 className="text-gray-900 font-display font-bold text-base">Upcoming scheduled changes</h2>
            <Link
              to="/schedule"
              className="flex items-center gap-1.5 text-indigo-600 hover:text-indigo-700 text-sm font-bold transition-colors"
            >
              View all <ArrowRight className="w-4 h-4" />
            </Link>
          </div>
          <div className="divide-y divide-gray-50">
            {pendingChanges.slice(0, 5).map(sc => (
              <div key={sc.id} className="flex items-center gap-4 px-8 py-4 hover:bg-indigo-50/20 transition-colors">
                <CalendarClock className="w-4 h-4 text-indigo-400 shrink-0" />
                <div className="flex-1 min-w-0">
                  <span className="font-mono text-sm font-semibold text-brand-600">{sc.flag_key}</span>
                  <span className="text-gray-400 text-xs ml-2">
                    {Object.entries(sc.patch)
                      .map(([k, v]) => `${k} → ${JSON.stringify(v)}`)
                      .join(', ')}
                  </span>
                </div>
                <span className="text-xs text-gray-400 shrink-0 tabular-nums">
                  {new Date(sc.scheduled_at).toLocaleString()}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
