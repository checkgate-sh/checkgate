import { useState, useEffect, useRef } from 'react'
import { useNavigate, Link } from 'react-router-dom'
import { Eye, EyeOff, LogIn, Clock } from 'lucide-react'
import { useAuth } from '../context/AuthContext'

export default function Login() {
  const { login } = useAuth()
  const navigate = useNavigate()
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [showPassword, setShowPassword] = useState(false)
  const [error, setError] = useState('')
  const [attemptsRemaining, setAttemptsRemaining] = useState<number | null>(null)
  const [loading, setLoading] = useState(false)
  const [workspaceName, setWorkspaceName] = useState('')
  const [lockoutSeconds, setLockoutSeconds] = useState(0)
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null)

  // Fetch workspace name to personalise the page.
  useEffect(() => {
    fetch('/api/auth/workspace', { credentials: 'same-origin' })
      .then(res => (res.ok ? (res.json() as Promise<{ workspace_name: string }>) : null))
      .then(data => {
        if (data?.workspace_name) setWorkspaceName(data.workspace_name)
      })
      .catch(() => {/* non-critical */})
  }, [])

  // Lockout countdown timer.
  useEffect(() => {
    if (lockoutSeconds <= 0) return
    timerRef.current = setInterval(() => {
      setLockoutSeconds(s => {
        if (s <= 1) {
          if (timerRef.current) clearInterval(timerRef.current)
          return 0
        }
        return s - 1
      })
    }, 1000)
    return () => {
      if (timerRef.current) clearInterval(timerRef.current)
    }
  }, [lockoutSeconds])

  const isLocked = lockoutSeconds > 0

  function formatLockout(seconds: number) {
    const m = Math.floor(seconds / 60)
    const s = seconds % 60
    return m > 0 ? `${m}m ${s}s` : `${s}s`
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (isLocked) return
    setError('')
    setAttemptsRemaining(null)
    setLoading(true)
    const result = await login(email.trim(), password)
    setLoading(false)

    if (result.ok) {
      navigate('/')
      return
    }

    if (result.retryAfterSeconds != null) {
      setLockoutSeconds(result.retryAfterSeconds)
      setError('Too many failed attempts.')
      return
    }

    setError(result.error ?? 'Login failed.')
    if (result.attemptsRemaining != null) {
      setAttemptsRemaining(result.attemptsRemaining)
    }
  }

  return (
    <div className="min-h-screen bg-gray-50 flex">
      {/* Left branding panel */}
      <div className="hidden lg:flex lg:w-5/12 flex-col justify-between p-12 xl:p-20 bg-white border-r border-gray-200">
        <div className="flex items-center gap-3">
          <div className="bg-white p-1 rounded-xl shadow-sm border border-gray-50 overflow-hidden">
            <img src="/logo.svg" alt="" className="h-7 w-7 object-contain" />
          </div>
          <span className="text-gray-900 font-display font-bold text-2xl tracking-tight">Checkgate</span>
        </div>

        <div className="max-w-lg">
          <blockquote className="text-4xl font-display font-bold text-gray-900 leading-[1.1] mb-6 tracking-tight">
            Ship with confidence. <span className="text-brand-600">Roll back in seconds.</span>
          </blockquote>
          <p className="text-gray-500 text-lg font-medium leading-relaxed">
            Self-hosted feature flags built for teams that move fast.
          </p>
        </div>

        <div className="flex items-center gap-3 text-gray-400 text-sm font-medium">
          <span className="px-2 py-1 bg-gray-50 rounded-md border border-gray-100">Apache 2.0 Licensed</span>
          <span className="w-1 h-1 bg-gray-300 rounded-full" />
          <span>Checkgate</span>
        </div>
      </div>

      {/* Right form panel */}
      <div className="flex-1 flex flex-col items-center justify-center p-8 bg-gray-50/50">
        {/* Mobile logo */}
        <div className="flex items-center gap-3 mb-8 lg:hidden">
          <div className="bg-white p-1 rounded-xl shadow-sm border border-gray-50 overflow-hidden">
            <img src="/logo.svg" alt="" className="h-6 w-6 object-contain" />
          </div>
          <span className="text-gray-900 font-display font-bold text-xl tracking-tight">Checkgate</span>
        </div>

        <div className="w-full max-w-lg bg-white p-10 rounded-3xl shadow-premium-xl border border-gray-50">
          <h2 className="text-3xl font-display font-bold text-gray-900 tracking-tight mb-1 text-center sm:text-left">
            Welcome back
          </h2>
          {workspaceName ? (
            <p className="text-brand-600 text-sm font-semibold mb-1 text-center sm:text-left">{workspaceName}</p>
          ) : null}
          <p className="text-gray-400 text-sm mb-10 text-center sm:text-left font-medium">Sign in to your control plane.</p>

          {/* Lockout banner */}
          {isLocked && (
            <div className="mb-5 p-3.5 rounded-lg bg-amber-50 border border-amber-200 flex items-center gap-3">
              <Clock className="w-4 h-4 text-amber-500 shrink-0" />
              <div>
                <p className="text-amber-700 text-sm font-semibold">Account temporarily locked</p>
                <p className="text-amber-600 text-xs mt-0.5">
                  Try again in <span className="font-bold tabular-nums">{formatLockout(lockoutSeconds)}</span>
                </p>
              </div>
            </div>
          )}

          {/* Error banner */}
          {!isLocked && error && (
            <div className="mb-5 p-3 rounded-lg bg-red-50 border border-red-200 text-red-600 text-sm">
              <p>{error}</p>
              {attemptsRemaining != null && attemptsRemaining > 0 && (
                <p className="mt-1 text-xs text-red-500">
                  {attemptsRemaining} attempt{attemptsRemaining !== 1 ? 's' : ''} remaining before lockout.
                </p>
              )}
            </div>
          )}

          <form onSubmit={e => void handleSubmit(e)} className="space-y-4">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1.5">
                Email address
              </label>
              <input
                type="email"
                required
                autoComplete="email"
                disabled={isLocked}
                value={email}
                onChange={e => { setEmail(e.target.value); setError(''); setAttemptsRemaining(null) }}
                placeholder="jane@acme.com"
                className="w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium disabled:opacity-50 disabled:cursor-not-allowed"
              />
            </div>

            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1.5">
                Password
              </label>
              <div className="relative">
                <input
                  type={showPassword ? 'text' : 'password'}
                  required
                  autoComplete="current-password"
                  disabled={isLocked}
                  value={password}
                  onChange={e => { setPassword(e.target.value); setError(''); setAttemptsRemaining(null) }}
                  placeholder="Your password"
                  className="w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 pr-10 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium disabled:opacity-50 disabled:cursor-not-allowed"
                />
                <button
                  type="button"
                  onClick={() => setShowPassword(s => !s)}
                  disabled={isLocked}
                  className="absolute right-3 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-700 transition-colors disabled:opacity-50"
                  aria-label={showPassword ? 'Hide password' : 'Show password'}
                >
                  {showPassword ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
                </button>
              </div>
            </div>

            <button
              type="submit"
              disabled={loading || isLocked || !email.trim() || !password}
              className="w-full flex items-center justify-center gap-2 py-3.5 px-4 bg-brand-600 hover:bg-brand-700 disabled:opacity-50 disabled:cursor-not-allowed text-white font-bold rounded-xl transition-all shadow-lg shadow-brand-200 hover:shadow-brand-300 hover:-translate-y-0.5 text-sm mt-4"
            >
              {loading ? (
                <span className="inline-block w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
              ) : (
                <LogIn className="w-4 h-4" />
              )}
              {loading ? 'Signing in…' : isLocked ? `Locked (${formatLockout(lockoutSeconds)})` : 'Sign in'}
            </button>
          </form>

          <p className="mt-6 text-center text-sm text-gray-500">
            No account yet?{' '}
            <Link to="/setup" className="text-brand-600 hover:text-brand-700 font-medium transition-colors">
              Run setup
            </Link>
          </p>
        </div>
      </div>
    </div>
  )
}
