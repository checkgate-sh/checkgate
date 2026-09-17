import { useState, useEffect } from 'react'
import { useNavigate, Link } from 'react-router-dom'
import { Flag, ArrowRight, CheckCircle2, Shield, Zap, Globe, Copy, Check, AlertCircle, Eye, EyeOff, Building2 } from 'lucide-react'
import { useAuth } from '../context/AuthContext'

const STEPS = ['Welcome', 'Workspace', 'Your account', 'SDK key', 'Done'] as const

const FEATURES = [
  { icon: Zap, title: 'Sub-microsecond evaluation', desc: 'Flags evaluated in-process — no network calls.' },
  { icon: Globe, title: 'Real-time updates', desc: 'SSE push propagates changes in < 50 ms.' },
  { icon: Shield, title: 'Self-hosted & private', desc: 'Your data never leaves your infrastructure.' },
]

export default function Setup() {
  const { completeSetup } = useAuth()
  const navigate = useNavigate()
  const [step, setStep] = useState(0)

  // Step 1: Workspace + first project
  const [workspaceName, setWorkspaceName] = useState('')
  const [projectName, setProjectName] = useState('')

  // Step 2: Account
  const [name, setName] = useState('')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')
  const [showPassword, setShowPassword] = useState(false)
  const [showConfirm, setShowConfirm] = useState(false)

  // Step 3: SDK key
  const [sdkKey, setSdkKey] = useState('')
  const [keyLoading, setKeyLoading] = useState(false)
  const [keyError, setKeyError] = useState('')
  const [copied, setCopied] = useState(false)
  const [confirmed, setConfirmed] = useState(false)

  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)

  // Fetch the generated key when the user reaches step 3.
  useEffect(() => {
    if (step !== 3 || sdkKey) return
    setKeyLoading(true)
    setKeyError('')
    fetch('/api/setup/key', { credentials: 'same-origin' })
      .then(async res => {
        if (!res.ok) throw new Error(`Server returned ${res.status}`)
        const data = await res.json() as { key: string }
        setSdkKey(data.key)
      })
      .catch(() => setKeyError('Could not load the SDK key. Is the server running?'))
      .finally(() => setKeyLoading(false))
  }, [step, sdkKey])

  async function handleCopy() {
    await navigator.clipboard.writeText(sdkKey)
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }

  async function handleFinish() {
    setError('')
    setLoading(true)
    const result = await completeSetup(workspaceName.trim(), projectName.trim() || 'My App', name.trim(), email.trim(), password)
    setLoading(false)
    if (!result.ok) {
      setError(result.error ?? 'Setup failed.')
      return
    }
    navigate('/')
  }

  const passwordsMatch = password === confirmPassword
  const accountValid =
    name.trim().length > 0 &&
    email.trim().length > 0 &&
    password.length >= 8 &&
    passwordsMatch

  return (
    <div className="min-h-screen bg-gray-50 flex">
      {/* Left panel */}
      <div className="hidden lg:flex lg:w-1/2 flex-col justify-between p-12 xl:p-20 bg-white border-r border-gray-200">
        <div className="flex items-center gap-3">
          <div className="bg-white p-1 rounded-xl shadow-sm border border-gray-50 overflow-hidden">
            <img src="/logo.svg" alt="" className="h-7 w-7 object-contain" />
          </div>
          <span className="text-gray-900 font-display font-bold text-2xl tracking-tight">Checkgate</span>
        </div>

        <div className="max-w-lg">
          <h1 className="text-4xl font-display font-bold text-gray-900 leading-[1.15] mb-6 tracking-tight">
            Feature flags built for the <span className="text-brand-600">modern web.</span>
          </h1>
          <p className="text-gray-500 text-lg font-medium leading-relaxed mb-10">
            Self-hosted, open-source, and built in Rust. Ship faster — without the vendor tax.
          </p>
          <div className="space-y-5">
            {FEATURES.map(({ icon: Icon, title, desc }) => (
              <div key={title} className="flex gap-4">
                <div className="w-9 h-9 rounded-lg bg-brand-50 border border-brand-100 flex items-center justify-center shrink-0">
                  <Icon className="w-4 h-4 text-brand-600" />
                </div>
                <div>
                  <p className="text-gray-900 font-medium text-sm">{title}</p>
                  <p className="text-gray-500 text-sm">{desc}</p>
                </div>
              </div>
            ))}
          </div>
        </div>

        <p className="text-gray-400 text-sm">Apache 2.0 Licensed · Open Source</p>
      </div>

      {/* Right panel */}
      <div className="flex-1 flex flex-col items-center justify-center p-8">
        {/* Step indicators */}
        <div className="flex items-center gap-2 mb-10">
          {STEPS.map((label, i) => (
            <div key={label} className="flex items-center gap-2">
              <div className={`w-8 h-8 rounded-xl flex items-center justify-center text-xs font-bold transition-all shadow-sm ${
                i < step ? 'bg-brand-600 text-white shadow-brand-200' :
                i === step ? 'bg-brand-50 text-brand-600 ring-2 ring-brand-500/20' :
                'bg-gray-100 text-gray-400'
              }`}>
                {i < step ? <CheckCircle2 className="w-4 h-4" /> : i + 1}
              </div>
              {i < STEPS.length - 1 && (
                <div className={`w-10 h-1 ${i < step ? 'bg-brand-600' : 'bg-gray-100'} rounded-full`} />
              )}
            </div>
          ))}
        </div>

        <div className="w-full max-w-lg bg-white p-12 rounded-3xl shadow-premium-xl border border-gray-50">
          {/* Step 0: Welcome */}
          {step === 0 && (
            <div className="text-center">
              <div className="w-14 h-14 rounded-2xl bg-brand-50 border border-brand-100 flex items-center justify-center mx-auto mb-6">
                <Flag className="w-7 h-7 text-brand-600" />
              </div>
              <h2 className="text-2xl font-bold text-gray-900 mb-2">Welcome to Checkgate</h2>
              <p className="text-gray-500 mb-8">
                Let's set up your control plane in under a minute. You'll configure your workspace, create an admin account, and get your SDK key.
              </p>
              <button
                onClick={() => setStep(1)}
                className="w-full flex items-center justify-center gap-2 py-3.5 px-4 bg-brand-600 hover:bg-brand-700 text-white font-bold rounded-xl transition-all shadow-lg shadow-brand-200 hover:shadow-brand-300 hover:-translate-y-0.5"
              >
                Get started <ArrowRight className="w-4 h-4" />
              </button>
              <p className="mt-6 text-sm text-gray-500">
                Already set up?{' '}
                <Link to="/login" className="text-brand-600 hover:text-brand-700 font-medium transition-colors">
                  Sign in
                </Link>
              </p>
            </div>
          )}

          {/* Step 1: Workspace */}
          {step === 1 && (
            <div>
              <div className="w-12 h-12 rounded-xl bg-brand-50 border border-brand-100 flex items-center justify-center mb-5">
                <Building2 className="w-6 h-6 text-brand-600" />
              </div>
              <h2 className="text-2xl font-bold text-gray-900 mb-1">Name your workspace</h2>
              <p className="text-gray-500 mb-6 text-sm">This is usually your company or team name. It'll appear in the dashboard header.</p>
              <div className="space-y-4">
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-1.5">Company / workspace name</label>
                  <input
                    autoFocus
                    type="text"
                    value={workspaceName}
                    onChange={e => setWorkspaceName(e.target.value)}
                    placeholder="Acme Corp"
                    className="w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
                  />
                </div>
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-1.5">First project name</label>
                  <input
                    type="text"
                    value={projectName}
                    onChange={e => setProjectName(e.target.value)}
                    placeholder="My App"
                    className="w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
                  />
                  <p className="mt-1.5 text-xs text-gray-400">You can add more projects later. Leave blank to use "My App".</p>
                </div>
              </div>
              <div className="flex gap-3 mt-6">
                <button onClick={() => setStep(0)} className="flex-1 py-2.5 px-4 bg-gray-100 hover:bg-gray-200 text-gray-600 font-medium rounded-lg transition-colors text-sm">
                  Back
                </button>
                <button
                  onClick={() => { if (workspaceName.trim()) setStep(2) }}
                  disabled={!workspaceName.trim()}
                  className="flex-1 flex items-center justify-center gap-2 py-2.5 px-4 bg-brand-600 hover:bg-brand-700 disabled:opacity-40 disabled:cursor-not-allowed text-white font-bold rounded-xl transition-all"
                >
                  Continue <ArrowRight className="w-4 h-4" />
                </button>
              </div>
            </div>
          )}

          {/* Step 2: Account details */}
          {step === 2 && (
            <div>
              <h2 className="text-2xl font-bold text-gray-900 mb-1">Create your admin account</h2>
              <p className="text-gray-500 mb-6 text-sm">Use your work email. Your password must be at least 8 characters.</p>
              <div className="space-y-4">
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-1.5">Full name</label>
                  <input
                    type="text"
                    value={name}
                    onChange={e => setName(e.target.value)}
                    placeholder="Jane Smith"
                    className="w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
                  />
                </div>
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-1.5">Work email</label>
                  <input
                    type="email"
                    value={email}
                    onChange={e => setEmail(e.target.value)}
                    placeholder="jane@acme.com"
                    className="w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
                  />
                </div>
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-1.5">Password</label>
                  <div className="relative">
                    <input
                      type={showPassword ? 'text' : 'password'}
                      value={password}
                      onChange={e => setPassword(e.target.value)}
                      placeholder="Min. 8 characters"
                      className="w-full bg-white border border-gray-100 rounded-xl px-4 py-2.5 pr-10 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 focus:ring-brand-500/10 focus:border-brand-500/30 transition-all shadow-premium"
                    />
                    <button
                      type="button"
                      onClick={() => setShowPassword(s => !s)}
                      className="absolute right-3 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-700 transition-colors"
                    >
                      {showPassword ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
                    </button>
                  </div>
                </div>
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-1.5">Confirm password</label>
                  <div className="relative">
                    <input
                      type={showConfirm ? 'text' : 'password'}
                      value={confirmPassword}
                      onChange={e => setConfirmPassword(e.target.value)}
                      placeholder="Repeat your password"
                      className={`w-full bg-white border rounded-xl px-4 py-2.5 pr-10 text-gray-900 placeholder-gray-400 text-sm focus:outline-none focus:ring-4 transition-all shadow-premium ${
                        confirmPassword && !passwordsMatch
                          ? 'border-rose-300 focus:ring-rose-500/10 focus:border-rose-400'
                          : 'border-gray-100 focus:ring-brand-500/10 focus:border-brand-500/30'
                      }`}
                    />
                    <button
                      type="button"
                      onClick={() => setShowConfirm(s => !s)}
                      className="absolute right-3 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-700 transition-colors"
                    >
                      {showConfirm ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
                    </button>
                  </div>
                  {confirmPassword && !passwordsMatch && (
                    <p className="mt-1.5 text-xs text-rose-500">Passwords don't match.</p>
                  )}
                </div>
              </div>
              <div className="flex gap-3 mt-6">
                <button onClick={() => setStep(1)} className="flex-1 py-2.5 px-4 bg-gray-100 hover:bg-gray-200 text-gray-600 font-medium rounded-lg transition-colors text-sm">
                  Back
                </button>
                <button
                  onClick={() => { if (accountValid) setStep(3) }}
                  disabled={!accountValid}
                  className="flex-1 flex items-center justify-center gap-2 py-2.5 px-4 bg-brand-600 hover:bg-brand-700 disabled:opacity-40 disabled:cursor-not-allowed text-white font-bold rounded-xl transition-all"
                >
                  Continue <ArrowRight className="w-4 h-4" />
                </button>
              </div>
            </div>
          )}

          {/* Step 3: SDK key — auto-generated by the server */}
          {step === 3 && (
            <div>
              <h2 className="text-2xl font-bold text-gray-900 mb-1">Your SDK key</h2>
              <p className="text-gray-500 mb-6 text-sm">
                This key authenticates SDK clients (mobile, web, server).
                <strong className="text-gray-700"> Save it now — it won't be shown again.</strong>
              </p>

              {keyError && (
                <div className="mb-4 p-3 rounded-lg bg-rose-50 border border-rose-100 text-rose-600 text-sm flex items-center gap-2">
                  <AlertCircle className="w-4 h-4 shrink-0" /> {keyError}
                </div>
              )}

              {error && (
                <div className="mb-4 p-3 rounded-lg bg-rose-50 border border-rose-100 text-rose-600 text-sm">
                  {error}
                </div>
              )}

              <div className="mb-4">
                <label className="block text-sm font-medium text-gray-700 mb-1.5">SDK Key</label>
                <div className="flex items-center gap-2 p-3 rounded-lg bg-gray-50 border border-gray-200">
                  {keyLoading ? (
                    <span className="inline-block w-4 h-4 border-2 border-gray-200 border-t-brand-500 rounded-full animate-spin" />
                  ) : (
                    <code className="flex-1 text-brand-600 text-sm font-mono break-all leading-relaxed">
                      {sdkKey || '—'}
                    </code>
                  )}
                  <button
                    type="button"
                    onClick={() => void handleCopy()}
                    disabled={!sdkKey}
                    className="shrink-0 p-1.5 rounded-md bg-white border border-gray-200 hover:bg-gray-50 text-gray-400 hover:text-gray-700 disabled:opacity-40 disabled:cursor-not-allowed transition-colors shadow-sm"
                    title="Copy to clipboard"
                  >
                    {copied ? <Check className="w-4 h-4 text-brand-500" /> : <Copy className="w-4 h-4" />}
                  </button>
                </div>
              </div>

              <label className="flex items-start gap-3 cursor-pointer select-none mb-6">
                <input
                  type="checkbox"
                  checked={confirmed}
                  onChange={e => setConfirmed(e.target.checked)}
                  className="mt-0.5 accent-brand-500"
                />
                <span className="text-sm text-gray-500">
                  I've copied and saved my SDK key. I understand it won't be shown again.
                </span>
              </label>

              <div className="flex gap-3">
                <button onClick={() => setStep(2)} className="flex-1 py-3 px-4 bg-gray-100 hover:bg-gray-200 text-gray-600 font-bold rounded-xl transition-all text-sm">
                  Back
                </button>
                <button
                  onClick={() => void handleFinish()}
                  disabled={loading || !confirmed || !sdkKey}
                  className="flex-1 flex items-center justify-center gap-2 py-3.5 px-4 bg-brand-600 hover:bg-brand-700 disabled:opacity-60 text-white font-bold rounded-xl transition-all shadow-lg shadow-brand-200 hover:shadow-brand-300 hover:-translate-y-0.5"
                >
                  {loading
                    ? <span className="inline-block w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                    : <><CheckCircle2 className="w-4 h-4" /> Finish setup</>}
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
