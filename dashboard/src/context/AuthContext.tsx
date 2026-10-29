import {
  createContext,
  useContext,
  useState,
  useCallback,
  useEffect,
  type ReactNode,
} from 'react'
import type { User } from '../types'

// ---------------------------------------------------------------------------
// Storage keys
// ---------------------------------------------------------------------------
const KEY_SETUP = 'lg_setup_complete'

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------
export interface Session {
  user: User
  workspaceName: string
}

export interface LoginResult {
  ok: boolean
  error?: string
  attemptsRemaining?: number
  retryAfterSeconds?: number
}

interface AuthContextValue {
  session: Session | null
  sessionLoading: boolean
  isSetupComplete: boolean
  setupLoading: boolean
  login: (email: string, password: string) => Promise<LoginResult>
  logout: () => Promise<void>
  completeSetup: (
    workspaceName: string,
    projectName: string,
    name: string,
    email: string,
    password: string,
    setupToken: string,
  ) => Promise<{ ok: boolean; error?: string }>
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

interface AuthResponse {
  email: string
  name: string
  role: string
  workspace_name: string
  is_setup_complete: boolean
}

function parseSession(body: AuthResponse): Session {
  const user: User = {
    id: body.email,
    email: body.email,
    name: body.name,
    role: body.role as User['role'],
    createdAt: '',
  }
  return { user, workspaceName: body.workspace_name }
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------
const AuthContext = createContext<AuthContextValue | null>(null)

export function AuthProvider({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<Session | null>(null)
  const [sessionLoading, setSessionLoading] = useState(true)
  const [isSetupComplete, setIsSetupComplete] = useState(
    () => localStorage.getItem(KEY_SETUP) === 'true',
  )
  // True until the authoritative /api/auth/workspace check resolves. Routing
  // must not act on `isSetupComplete` while this is true — its initial value
  // above is only a localStorage guess (wrong for a fresh browser, a
  // different device, or a session that expired after a server restart), and
  // routing to /setup on a wrong guess is a one-way trip: nothing on the
  // /setup route itself re-checks and bounces back once the real value
  // arrives.
  const [setupLoading, setSetupLoading] = useState(true)

  // On mount: restore session and authoritative setup state from the server.
  //
  // Setup status is fetched from the *public* /api/auth/workspace endpoint,
  // not just inferred from /api/auth/me — the latter 401s for anyone without
  // a valid session cookie (a fresh browser, a different device, a session
  // that expired after a server restart), which previously meant those users
  // got permanently bounced to /setup instead of /login even though setup
  // had already been completed by someone else.
  useEffect(() => {
    fetch('/api/auth/workspace', { credentials: 'same-origin' })
      .then(res => (res.ok ? res.json() as Promise<{ workspace_name: string; is_setup_complete: boolean }> : null))
      .then(body => {
        if (body) {
          setIsSetupComplete(body.is_setup_complete)
          if (body.is_setup_complete) localStorage.setItem(KEY_SETUP, 'true')
          else localStorage.removeItem(KEY_SETUP)
        }
      })
      .catch(() => {/* network error — fall back to the localStorage-seeded initial state */})
      .finally(() => setSetupLoading(false))

    fetch('/api/auth/me', { credentials: 'same-origin' })
      .then(res => {
        if (res.ok) return res.json() as Promise<AuthResponse>
        return null
      })
      .then(body => {
        if (body) {
          setSession(parseSession(body))
          setIsSetupComplete(body.is_setup_complete)
          if (body.is_setup_complete) localStorage.setItem(KEY_SETUP, 'true')
        }
      })
      .catch(() => {/* network error — stay logged out */})
      .finally(() => setSessionLoading(false))
  }, [])

  const login = useCallback(async (email: string, password: string): Promise<LoginResult> => {
    try {
      const res = await fetch('/api/auth/login', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json', 'X-Checkgate-Request': 'true' },
        body: JSON.stringify({ email: email.trim(), password }),
      })

      if (res.status === 429) {
        const body = await res.json() as { error: string; retry_after_seconds: number }
        return { ok: false, error: body.error, retryAfterSeconds: body.retry_after_seconds }
      }
      if (res.status === 401) {
        const body = await res.json() as { error: string; attempts_remaining?: number }
        return { ok: false, error: body.error, attemptsRemaining: body.attempts_remaining }
      }
      if (!res.ok) {
        return { ok: false, error: `Server error (${res.status}). Please try again.` }
      }

      const body = await res.json() as AuthResponse
      setSession(parseSession(body))
      return { ok: true }
    } catch {
      return { ok: false, error: 'Could not reach the server.' }
    }
  }, [])

  const logout = useCallback(async () => {
    try {
      await fetch('/api/auth/logout', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'X-Checkgate-Request': 'true' },
      })
    } catch {
      // Best-effort — clear local state regardless.
    }
    setSession(null)
  }, [])

  const completeSetup = useCallback(
    async (workspaceName: string, projectName: string, name: string, email: string, password: string, setupToken: string) => {
      try {
        const res = await fetch('/api/setup/complete', {
          method: 'POST',
          credentials: 'same-origin',
          headers: {
            'Content-Type': 'application/json',
            'X-Checkgate-Request': 'true',
            'X-Checkgate-Setup-Token': setupToken,
          },
          body: JSON.stringify({
            workspace_name: workspaceName.trim(),
            project_name: projectName.trim() || 'My App',
            name: name.trim(),
            email: email.trim(),
            password,
          }),
        })

        if (res.status === 404) {
          return { ok: false, error: 'Setup already complete. Please log in.' }
        }
        if (res.status === 401) {
          return { ok: false, error: 'The setup token is not valid. Go back to the first step and re-enter it.' }
        }
        if (res.status === 409) {
          return { ok: false, error: 'An account with this email already exists. Use a different email.' }
        }
        if (res.status === 422) {
          return { ok: false, error: 'Password must be at least 8 characters.' }
        }
        if (!res.ok) {
          return { ok: false, error: `Server returned ${res.status}.` }
        }

        const body = await res.json() as AuthResponse
        localStorage.setItem(KEY_SETUP, 'true')
        setIsSetupComplete(true)
        setSession(parseSession(body))
        return { ok: true }
      } catch {
        return { ok: false, error: 'Could not reach the server.' }
      }
    },
    [],
  )

  return (
    <AuthContext.Provider
      value={{
        session,
        sessionLoading,
        isSetupComplete,
        setupLoading,
        login,
        logout,
        completeSetup,
      }}
    >
      {children}
    </AuthContext.Provider>
  )
}

export function useAuth() {
  const ctx = useContext(AuthContext)
  if (!ctx) throw new Error('useAuth must be used inside AuthProvider')
  return ctx
}
