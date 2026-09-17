import { useCallback, useEffect, useState } from 'react'
import { GitPullRequest, CheckCircle2, XCircle, Ban, Clock } from 'lucide-react'
import { changeRequestsApi, type ChangeRequest, type ChangeRequestStatus } from '../api'
import { useEnvironment } from '../context/EnvironmentContext'
import { useAuth } from '../context/AuthContext'

const STATUS_LABEL: Record<ChangeRequestStatus, string> = {
  pending: 'Pending',
  approved: 'Approved',
  rejected: 'Rejected',
  cancelled: 'Cancelled',
}

const STATUS_STYLE: Record<ChangeRequestStatus, string> = {
  pending: 'bg-amber-50 text-amber-700 border-amber-100',
  approved: 'bg-emerald-50 text-emerald-700 border-emerald-100',
  rejected: 'bg-rose-50 text-rose-700 border-rose-100',
  cancelled: 'bg-gray-50 text-gray-500 border-gray-200',
}

function StatusBadge({ status }: { status: ChangeRequestStatus }) {
  return (
    <span className={`px-1.5 py-0.5 text-[10px] font-bold rounded uppercase tracking-wide border ${STATUS_STYLE[status]}`}>
      {STATUS_LABEL[status]}
    </span>
  )
}

function PatchSummary({ patch }: { patch: Record<string, unknown> }) {
  const fields = Object.keys(patch)
  if (fields.length === 0) return <span className="text-gray-400 text-xs">no fields</span>
  return (
    <div className="flex flex-wrap gap-1">
      {fields.map(f => (
        <span key={f} className="px-1.5 py-0.5 bg-gray-100 text-gray-600 rounded text-[10px] font-mono">
          {f}: {JSON.stringify(patch[f])}
        </span>
      ))}
    </div>
  )
}

export default function ChangeRequests() {
  const { activeEnv } = useEnvironment()
  const { session } = useAuth()
  const isAdmin = session?.user.role === 'admin'
  const [status, setStatus] = useState<ChangeRequestStatus | ''>('pending')
  const [requests, setRequests] = useState<ChangeRequest[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [actingId, setActingId] = useState<number | null>(null)

  const load = useCallback(async () => {
    if (!activeEnv) return
    setLoading(true)
    setError(null)
    try {
      setRequests(await changeRequestsApi.list(activeEnv.id, status || undefined))
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load change requests')
    } finally {
      setLoading(false)
    }
  }, [activeEnv, status])

  useEffect(() => { void load() }, [load])

  async function handleApprove(id: number) {
    if (!activeEnv) return
    setActingId(id)
    try {
      await changeRequestsApi.approve(activeEnv.id, id)
      await load()
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Approve failed')
    } finally {
      setActingId(null)
    }
  }

  async function handleReject(id: number) {
    if (!activeEnv) return
    const reason = prompt('Reason for rejecting (optional):') ?? undefined
    setActingId(id)
    try {
      await changeRequestsApi.reject(activeEnv.id, id, reason || undefined)
      await load()
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Reject failed')
    } finally {
      setActingId(null)
    }
  }

  async function handleCancel(id: number) {
    if (!activeEnv) return
    if (!confirm('Withdraw this change request?')) return
    setActingId(id)
    try {
      await changeRequestsApi.cancel(activeEnv.id, id)
      await load()
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Cancel failed')
    } finally {
      setActingId(null)
    }
  }

  return (
    <div className="w-full space-y-5">
      <div className="premium-card shadow-premium-lg border-none">
        <div className="flex items-start gap-5 px-6 py-5 border-b border-gray-50 bg-white">
          <div className="w-10 h-10 rounded-xl bg-brand-50 flex items-center justify-center shrink-0">
            <GitPullRequest className="w-5 h-5 text-brand-600" />
          </div>
          <div className="flex-1">
            <h2 className="text-gray-900 font-display font-bold text-sm tracking-tight">Change requests</h2>
            <p className="text-gray-400 text-xs mt-0.5">
              {activeEnv?.name ?? 'This environment'} requires approval before flag changes take effect.
              Someone other than the requester must review each one.
            </p>
          </div>
          <select
            value={status}
            onChange={e => setStatus(e.target.value as ChangeRequestStatus | '')}
            className="bg-white border border-gray-100 rounded-xl px-3 py-2 text-sm text-gray-700 focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 shadow-premium"
          >
            <option value="pending">Pending</option>
            <option value="approved">Approved</option>
            <option value="rejected">Rejected</option>
            <option value="cancelled">Cancelled</option>
            <option value="">All</option>
          </select>
        </div>

        <div className="p-6 space-y-3">
          {error && (
            <div className="p-3.5 rounded-lg bg-red-50 border border-red-200 text-red-600 text-sm">{error}</div>
          )}

          {loading ? (
            <div className="flex items-center justify-center h-24 text-gray-400 text-sm">Loading…</div>
          ) : requests.length === 0 ? (
            <p className="text-center text-gray-400 text-sm py-6">No {status || ''} change requests.</p>
          ) : (
            requests.map(cr => {
              const canReview = isAdmin || session?.user.email !== cr.requested_by
              const isSelf = session?.user.email === cr.requested_by
              return (
                <div key={cr.id} className="p-4 rounded-xl bg-white border border-gray-100 shadow-sm space-y-2.5">
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="font-mono text-sm text-gray-900 font-medium">{cr.flag_key}</span>
                        <StatusBadge status={cr.status} />
                      </div>
                      <p className="text-xs text-gray-400 mt-0.5">
                        Requested by {cr.requested_by} · {new Date(cr.created_at).toLocaleString()}
                        {cr.reviewed_by && ` · reviewed by ${cr.reviewed_by}`}
                      </p>
                      {cr.reason && (
                        <p className="text-xs text-rose-500 mt-1">Reason: {cr.reason}</p>
                      )}
                    </div>
                    {cr.status === 'pending' && (
                      <div className="flex items-center gap-1.5 shrink-0">
                        {isSelf && (
                          <button
                            onClick={() => void handleCancel(cr.id)}
                            disabled={actingId === cr.id}
                            title="Withdraw"
                            className="flex items-center gap-1 px-2.5 py-1.5 bg-gray-100 hover:bg-gray-200 text-gray-600 text-xs font-bold rounded-lg transition-colors disabled:opacity-50"
                          >
                            <Ban className="w-3.5 h-3.5" /> Withdraw
                          </button>
                        )}
                        {canReview && (
                          <>
                            <button
                              onClick={() => void handleReject(cr.id)}
                              disabled={actingId === cr.id}
                              title="Reject"
                              className="flex items-center gap-1 px-2.5 py-1.5 bg-rose-50 hover:bg-rose-100 text-rose-600 text-xs font-bold rounded-lg transition-colors disabled:opacity-50"
                            >
                              <XCircle className="w-3.5 h-3.5" /> Reject
                            </button>
                            <button
                              onClick={() => void handleApprove(cr.id)}
                              disabled={actingId === cr.id}
                              title="Approve"
                              className="flex items-center gap-1 px-2.5 py-1.5 bg-brand-600 hover:bg-brand-700 text-white text-xs font-bold rounded-lg transition-all disabled:opacity-50"
                            >
                              <CheckCircle2 className="w-3.5 h-3.5" /> Approve
                            </button>
                          </>
                        )}
                        {isSelf && !isAdmin && (
                          <span className="flex items-center gap-1 text-xs text-gray-400" title="You requested this — someone else must review it">
                            <Clock className="w-3.5 h-3.5" /> awaiting reviewer
                          </span>
                        )}
                      </div>
                    )}
                  </div>
                  <PatchSummary patch={cr.patch} />
                </div>
              )
            })
          )}
        </div>
      </div>
    </div>
  )
}
