import { NavLink, useNavigate, useLocation } from 'react-router-dom'
import {
  LayoutDashboard,
  ToggleLeft,
  Users,
  Settings,
  LogOut,
  Globe,
  ChevronDown,
  Activity,
  FolderKanban,
  Plus,
  Tags,
  History,
  Webhook,
  Wifi,
  CalendarClock,
  GitPullRequest,
  PieChart,
  FlaskConical,
  PanelLeftClose,
  PanelLeftOpen,
  ChevronRight,
  MessageSquare,
} from 'lucide-react'
import { useState, useRef, useEffect } from 'react'
import { useAuth } from '../context/AuthContext'
import { useEnvironment, type Environment } from '../context/EnvironmentContext'
import { useProject } from '../context/ProjectContext'
import type { Project } from '../types'

interface NavItem {
  to: string
  icon: typeof LayoutDashboard
  label: string
  end?: boolean
  adminOnly?: boolean
}

interface NavGroup {
  id: string
  /** Scope label shown in the section header. */
  label: string
  items: NavItem[]
}

/** Always visible, above the grouped sections — the one page with no scope. */
const HOME: NavItem = { to: '/', icon: LayoutDashboard, label: 'Dashboard', end: true }

/**
 * Nav grouped by the scope its data actually belongs to, so it's obvious which
 * switcher above changes what you're looking at. Membership follows the API
 * each page calls, not intuition:
 *
 *   environment — passes `activeEnv.id` (flags, segments, change requests,
 *                 scheduled, experiments, exposure, impressions, webhooks, audit)
 *   project     — passes `activeProject.id` (environments)
 *   workspace   — unscoped endpoints (projects, users, sdk health, settings)
 */
const NAV_GROUPS: NavGroup[] = [
  {
    id: 'environment',
    label: 'Environment',
    items: [
      { to: '/flags', icon: ToggleLeft, label: 'Feature Flags' },
      { to: '/segments', icon: Tags, label: 'Segments' },
      { to: '/change-requests', icon: GitPullRequest, label: 'Change Requests' },
      { to: '/schedule', icon: CalendarClock, label: 'Scheduled' },
      { to: '/experiments', icon: FlaskConical, label: 'Experiments' },
      { to: '/exposure', icon: PieChart, label: 'Exposure' },
      { to: '/impressions', icon: Activity, label: 'Impressions' },
      { to: '/audit', icon: History, label: 'Audit Log' },
      { to: '/webhooks', icon: Webhook, label: 'Webhooks', adminOnly: true },
      { to: '/integrations', icon: MessageSquare, label: 'Chat Alerts', adminOnly: true },
    ],
  },
  {
    id: 'project',
    label: 'Project',
    items: [
      { to: '/environments', icon: Globe, label: 'Environments', adminOnly: true },
    ],
  },
  {
    id: 'workspace',
    label: 'Workspace',
    items: [
      { to: '/projects', icon: FolderKanban, label: 'Projects', adminOnly: true },
      { to: '/users', icon: Users, label: 'Users', adminOnly: true },
      { to: '/sdk-health', icon: Wifi, label: 'SDK Health' },
      { to: '/settings', icon: Settings, label: 'Settings', adminOnly: true },
    ],
  },
]

// ---------------------------------------------------------------------------
// Project switcher dropdown
// ---------------------------------------------------------------------------

function ProjectSwitcher() {
  const { projects, activeProject, setActiveProject } = useProject()
  const { session } = useAuth()
  const navigate = useNavigate()
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const isAdmin = session?.user.role === 'admin'

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', handleClick)
    return () => document.removeEventListener('mousedown', handleClick)
  }, [])

  if (!activeProject) return null

  function select(p: Project) {
    setActiveProject(p)
    setOpen(false)
  }

  return (
    <div ref={ref} className="relative px-4 mb-1">
      <button
        onClick={() => setOpen(o => !o)}
        className="w-full flex items-center gap-2.5 px-3 py-2 bg-brand-50 hover:bg-brand-100 border border-brand-100 rounded-xl transition-colors text-left"
      >
        <FolderKanban className="w-4 h-4 text-brand-600 shrink-0" />
        <span className="flex-1 text-sm font-bold text-brand-800 truncate">{activeProject.name}</span>
        <ChevronDown className={`w-3.5 h-3.5 text-brand-400 transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>

      {open && (
        <div className="absolute left-4 right-4 top-full mt-1 z-50 bg-white border border-gray-100 rounded-xl shadow-lg overflow-hidden">
          {projects.map(p => (
            <button
              key={p.id}
              onClick={() => select(p)}
              className={`w-full flex items-center gap-2.5 px-3 py-2.5 text-sm transition-colors text-left hover:bg-gray-50 ${
                p.id === activeProject.id ? 'bg-brand-50 text-brand-700 font-semibold' : 'text-gray-700'
              }`}
            >
              <FolderKanban className="w-3.5 h-3.5 shrink-0 text-gray-400" />
              <span className="flex-1 truncate">{p.name}</span>
            </button>
          ))}
          {isAdmin && (
            <button
              onClick={() => { setOpen(false); navigate('/projects') }}
              className="w-full flex items-center gap-2.5 px-3 py-2.5 text-sm text-brand-600 hover:bg-brand-50 border-t border-gray-100 transition-colors"
            >
              <Plus className="w-3.5 h-3.5 shrink-0" />
              <span className="font-semibold">New project</span>
            </button>
          )}
        </div>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Environment switcher dropdown
// ---------------------------------------------------------------------------

function EnvSwitcher() {
  const { environments, activeEnv, setActiveEnv } = useEnvironment()
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', handleClick)
    return () => document.removeEventListener('mousedown', handleClick)
  }, [])

  if (!activeEnv) return null

  function select(env: Environment) {
    setActiveEnv(env)
    setOpen(false)
  }

  return (
    <div ref={ref} className="relative px-4 mb-3">
      <button
        onClick={() => setOpen(o => !o)}
        className="w-full flex items-center gap-2.5 px-3 py-2 bg-gray-50 hover:bg-gray-100 border border-gray-100 rounded-xl transition-colors text-left"
      >
        <div className="w-2.5 h-2.5 rounded-full shrink-0" style={{ backgroundColor: activeEnv.color }} />
        <span className="flex-1 text-sm font-semibold text-gray-700 truncate">{activeEnv.name}</span>
        <ChevronDown className={`w-3.5 h-3.5 text-gray-400 transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>

      {open && (
        <div className="absolute left-4 right-4 top-full mt-1 z-50 bg-white border border-gray-100 rounded-xl shadow-lg overflow-hidden">
          {environments.map(env => (
            <button
              key={env.id}
              onClick={() => select(env)}
              className={`w-full flex items-center gap-2.5 px-3 py-2.5 text-sm transition-colors text-left hover:bg-gray-50 ${
                env.id === activeEnv.id ? 'bg-brand-50 text-brand-700 font-semibold' : 'text-gray-700'
              }`}
            >
              <div className="w-2.5 h-2.5 rounded-full shrink-0" style={{ backgroundColor: env.color }} />
              <span className="flex-1 truncate">{env.name}</span>
              {env.is_default && (
                <span className="text-[9px] text-gray-400 uppercase tracking-wide font-bold">default</span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

const KEY_SIDEBAR_COLLAPSED = 'lg_sidebar_collapsed'
const KEY_CLOSED_GROUPS = 'lg_sidebar_closed_groups'

function NavItemLink({ item, collapsed }: { item: NavItem; collapsed: boolean }) {
  const { to, icon: Icon, label, end } = item
  return (
    <NavLink
      to={to}
      end={end}
      title={collapsed ? label : undefined}
      className={({ isActive }) =>
        `group flex items-center rounded-xl text-sm font-medium transition-all duration-200 ${
          collapsed ? 'justify-center px-0 py-2.5' : 'gap-3 px-3 py-2.5'
        } ${
          isActive
            ? 'bg-brand-50 text-brand-700 shadow-sm shadow-brand-100/50'
            : 'text-gray-500 hover:text-gray-900 hover:bg-gray-50'
        }`
      }
    >
      {({ isActive }) => (
        <>
          <div className={`p-1 rounded-lg transition-colors ${isActive ? 'bg-white shadow-sm' : 'group-hover:bg-white/50'}`}>
            <Icon className={`w-4 h-4 shrink-0 ${isActive ? 'text-brand-600' : 'text-gray-400 group-hover:text-gray-600'}`} />
          </div>
          {!collapsed && <span className="flex-1">{label}</span>}
        </>
      )}
    </NavLink>
  )
}

export default function Sidebar() {
  const { session, logout } = useAuth()
  const { activeProject } = useProject()
  const { activeEnv } = useEnvironment()
  const navigate = useNavigate()
  const { pathname } = useLocation()
  const isAdmin = session?.user.role === 'admin'
  const [collapsed, setCollapsed] = useState(() => localStorage.getItem(KEY_SIDEBAR_COLLAPSED) === 'true')
  // Admin/config sections start closed so the first impression is the handful
  // of pages people use daily, not all fifteen at once. Opening one sticks.
  const [closedGroups, setClosedGroups] = useState<string[]>(() => {
    try {
      const raw = localStorage.getItem(KEY_CLOSED_GROUPS)
      return raw ? JSON.parse(raw) as string[] : ['project', 'workspace']
    } catch {
      return ['project', 'workspace']
    }
  })

  const groups = NAV_GROUPS
    .map(g => ({ ...g, items: g.items.filter(i => !i.adminOnly || isAdmin) }))
    .filter(g => g.items.length > 0)

  /** Context name shown beside the scope label, so the header names what it applies to. */
  function groupContext(id: string): string | undefined {
    if (id === 'environment') return activeEnv?.name
    if (id === 'project') return activeProject?.name
    return undefined
  }

  function toggleGroup(id: string) {
    setClosedGroups(prev => {
      const next = prev.includes(id) ? prev.filter(g => g !== id) : [...prev, id]
      localStorage.setItem(KEY_CLOSED_GROUPS, JSON.stringify(next))
      return next
    })
  }

  function toggleCollapsed() {
    setCollapsed(c => {
      localStorage.setItem(KEY_SIDEBAR_COLLAPSED, String(!c))
      return !c
    })
  }

  function handleLogout() {
    logout()
    navigate('/login')
  }

  return (
    <aside className={`${collapsed ? 'w-[76px]' : 'w-64'} shrink-0 flex flex-col h-screen bg-white border-r border-gray-100 shadow-premium transition-[width] duration-200`}>
      {/* Logo + workspace name */}
      <div className={`flex items-center h-16 mb-2 ${collapsed ? 'justify-center px-2' : 'gap-3 px-6'}`}>
        <div className="bg-white p-1 rounded-xl shadow-sm border border-gray-50 overflow-hidden shrink-0">
          <img src="/logo.svg" alt="" className="h-8 w-8 object-contain" />
        </div>
        {!collapsed && (
          <div className="min-w-0">
            <span className="text-gray-900 font-display font-bold text-xl tracking-tight">Checkgate</span>
            {session?.workspaceName && (
              <p className="text-gray-400 text-[10px] font-medium truncate leading-none mt-0.5">{session.workspaceName}</p>
            )}
          </div>
        )}
      </div>

      {!collapsed && (
        <>
          {/* Project switcher */}
          <ProjectSwitcher />

          {/* Environment switcher */}
          <EnvSwitcher />
        </>
      )}

      {/* Nav */}
      <nav className={`flex-1 py-2 overflow-y-auto ${collapsed ? 'px-3 space-y-1' : 'px-4 space-y-1'}`}>
        <NavItemLink item={HOME} collapsed={collapsed} />

        {groups.map(group => {
          // A section holding the current page always renders open — collapsing
          // it would hide the very item marked active.
          const hasActive = group.items.some(i => pathname === i.to || pathname.startsWith(`${i.to}/`))
          const open = collapsed || hasActive || !closedGroups.includes(group.id)
          const context = groupContext(group.id)

          return (
            <div key={group.id} className={collapsed ? 'pt-1 mt-1 border-t border-gray-100' : 'pt-2'}>
              {!collapsed && (
                <button
                  onClick={() => toggleGroup(group.id)}
                  aria-expanded={open}
                  className="w-full flex items-center gap-1.5 px-3 py-1.5 text-[10px] font-bold uppercase tracking-widest text-gray-400 hover:text-gray-600 transition-colors"
                >
                  <ChevronRight className={`w-3 h-3 shrink-0 transition-transform ${open ? 'rotate-90' : ''}`} />
                  <span className="shrink-0">{group.label}</span>
                  {context && (
                    <>
                      <span className="text-gray-200">·</span>
                      <span className="truncate normal-case tracking-normal font-semibold text-gray-400">
                        {context}
                      </span>
                    </>
                  )}
                </button>
              )}

              {open && (
                <div className="space-y-1">
                  {group.items.map(item => (
                    <NavItemLink key={item.to} item={item} collapsed={collapsed} />
                  ))}
                </div>
              )}
            </div>
          )
        })}
      </nav>

      {/* Collapse toggle */}
      <div className={`px-4 pb-2 ${collapsed ? 'flex justify-center px-3' : ''}`}>
        <button
          onClick={toggleCollapsed}
          title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
          className={`flex items-center gap-3 py-2 rounded-xl text-xs font-semibold text-gray-400 hover:text-gray-700 hover:bg-gray-50 transition-all duration-200 ${
            collapsed ? 'justify-center w-10' : 'w-full px-3'
          }`}
        >
          {collapsed ? <PanelLeftOpen className="w-4 h-4 shrink-0" /> : <PanelLeftClose className="w-4 h-4 shrink-0" />}
          {!collapsed && <span>Collapse</span>}
        </button>
      </div>

      {/* User section */}
      <div className={`py-4 border-t border-gray-100 bg-gray-50/50 ${collapsed ? 'px-3' : 'px-4'}`}>
        <div className={`flex items-center mb-2 ${collapsed ? 'justify-center' : 'gap-3 px-2 py-2'}`}>
          <div className="w-9 h-9 rounded-xl bg-gradient-to-br from-brand-500 to-brand-600 shadow-md shadow-brand-200 flex items-center justify-center shrink-0" title={collapsed ? session?.user.name : undefined}>
            <span className="text-white text-sm font-bold">
              {session?.user.name.charAt(0).toUpperCase() ?? '?'}
            </span>
          </div>
          {!collapsed && (
            <div className="flex-1 min-w-0">
              <p className="text-gray-900 text-xs font-bold truncate">{session?.user.name}</p>
              <p className="text-gray-500 text-[10px] tracking-wider uppercase font-semibold">{session?.user.role}</p>
            </div>
          )}
        </div>
        <button
          onClick={handleLogout}
          title={collapsed ? 'Sign out' : undefined}
          className={`flex items-center rounded-xl text-sm text-gray-500 hover:text-rose-600 hover:bg-rose-50 transition-all duration-200 group ${
            collapsed ? 'justify-center w-10 h-10 mx-auto' : 'w-full gap-3 px-3 py-2.5'
          }`}
        >
          <LogOut className="w-4 h-4 transition-transform group-hover:translate-x-0.5 shrink-0" />
          {!collapsed && <span className="font-semibold">Sign out</span>}
        </button>
      </div>
    </aside>
  )
}
