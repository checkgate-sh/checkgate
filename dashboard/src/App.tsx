import { Routes, Route, Navigate, useParams } from 'react-router-dom'
import { useAuth } from './context/AuthContext'
import { ProjectProvider } from './context/ProjectContext'
import { EnvironmentProvider } from './context/EnvironmentContext'
import Layout from './components/Layout'
import Login from './pages/Login'
import Setup from './pages/Setup'
import AuditLog from './pages/AuditLog'
import Playground from './pages/Playground'
import Dashboard from './pages/Dashboard'
import FlagList from './pages/FlagList'
import Scheduled from './pages/Scheduled'
import SdkHealth from './pages/SdkHealth'
import Segments from './pages/Segments'
import Users from './pages/Users'
import Settings from './pages/Settings'
import Environments from './pages/Environments'
import EnvironmentDiff from './pages/EnvironmentDiff'
import ChangeRequests from './pages/ChangeRequests'
import Impressions from './pages/Impressions'
import Exposure from './pages/Exposure'
import Experiments from './pages/Experiments'
import Projects from './pages/Projects'
import ProjectSettings from './pages/ProjectSettings'
import Webhooks from './pages/Webhooks'
import Integrations from './pages/Integrations'

/**
 * Flag create/edit moved from standalone pages into a slide-over on the list.
 * These keep the old URLs working — bookmarks and links land on the list with
 * the panel already open on the right flag.
 */
function RedirectToFlagPanel({ mode }: { readonly mode: 'new' | 'edit' }) {
  const { key } = useParams<{ key?: string }>()
  if (mode === 'new') return <Navigate to="/flags?new=1" replace />
  if (!key) return <Navigate to="/flags" replace />
  return <Navigate to={`/flags?edit=${encodeURIComponent(key)}`} replace />
}

function RequireAuth({ children }: { readonly children: React.ReactNode }) {
  const { session, sessionLoading, isSetupComplete, setupLoading } = useAuth()
  // Must not act on isSetupComplete until the authoritative check resolves —
  // its pre-check value is only a localStorage guess, and routing to /setup
  // on a wrong guess is a one-way trip (nothing there re-checks and bounces
  // back once the real value arrives).
  if (setupLoading) return null
  if (!isSetupComplete) return <Navigate to="/setup" replace />
  if (sessionLoading) return null
  if (!session) return <Navigate to="/login" replace />
  return <>{children}</>
}

function PublicOnly({ children }: { readonly children: React.ReactNode }) {
  const { session, sessionLoading, isSetupComplete, setupLoading } = useAuth()
  if (setupLoading) return null
  if (!isSetupComplete) return <Navigate to="/setup" replace />
  if (sessionLoading) return null
  if (session) return <Navigate to="/" replace />
  return <>{children}</>
}

function SetupOnly({ children }: { readonly children: React.ReactNode }) {
  const { isSetupComplete, setupLoading } = useAuth()
  if (setupLoading) return null
  if (isSetupComplete) return <Navigate to="/login" replace />
  return <>{children}</>
}

export default function App() {
  return (
    <Routes>
      {/* Auth routes (no sidebar) */}
      <Route
        path="/setup"
        element={
          <SetupOnly>
            <Setup />
          </SetupOnly>
        }
      />
      <Route
        path="/login"
        element={
          <PublicOnly>
            <Login />
          </PublicOnly>
        }
      />

      {/* Protected app routes */}
      <Route
        path="/*"
        element={
          <RequireAuth>
            <ProjectProvider>
              <EnvironmentProvider>
                <Layout>
                  <Routes>
                    <Route path="/" element={<Dashboard />} />
                    <Route path="/flags" element={<FlagList />} />
                    <Route path="/flags/new" element={<RedirectToFlagPanel mode="new" />} />
                    <Route path="/flags/:key/edit" element={<RedirectToFlagPanel mode="edit" />} />
                    <Route path="/segments" element={<Segments />} />
                    <Route path="/change-requests" element={<ChangeRequests />} />
                    <Route path="/schedule" element={<Scheduled />} />
                    <Route path="/impressions" element={<Impressions />} />
                    <Route path="/exposure" element={<Exposure />} />
                    <Route path="/experiments" element={<Experiments />} />
                    <Route path="/audit" element={<AuditLog />} />
                    <Route path="/playground" element={<Playground />} />
                    <Route path="/sdk-health" element={<SdkHealth />} />
                    <Route path="/webhooks" element={<Webhooks />} />
                    <Route path="/integrations" element={<Integrations />} />
                    <Route path="/environments" element={<Environments />} />
                    <Route path="/environments/diff" element={<EnvironmentDiff />} />
                    <Route path="/projects" element={<Projects />} />
                    <Route path="/projects/:projectId" element={<ProjectSettings />} />
                    <Route path="/users" element={<Users />} />
                    <Route path="/settings" element={<Settings />} />
                    <Route path="*" element={<Navigate to="/" replace />} />
                  </Routes>
                </Layout>
              </EnvironmentProvider>
            </ProjectProvider>
          </RequireAuth>
        }
      />
    </Routes>
  )
}
