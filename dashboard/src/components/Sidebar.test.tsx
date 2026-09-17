import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { MemoryRouter } from 'react-router-dom'
import Sidebar from './Sidebar'

const mockAuthContext = {
  logout: vi.fn(),
  session: {
    user: {
      name: 'testuser',
      role: 'admin'
    }
  }
}

vi.mock('../context/AuthContext', () => ({
  useAuth: () => mockAuthContext,
}))

const ENV = { id: 'env-1', name: 'Development', color: '#4f46e5' }
const PROJECT = { id: 'proj-1', name: 'Bookspine' }

vi.mock('../context/EnvironmentContext', () => ({
  useEnvironment: () => ({
    environments: [ENV],
    activeEnv: ENV,
    setActiveEnv: vi.fn(),
  }),
}))

vi.mock('../context/ProjectContext', () => ({
  useProject: () => ({
    projects: [PROJECT],
    activeProject: PROJECT,
    setActiveProject: vi.fn(),
    loading: false,
    reload: vi.fn(),
  }),
}))

function renderSidebar(initialEntry = '/') {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <Sidebar />
    </MemoryRouter>,
  )
}

beforeEach(() => {
  localStorage.clear()
})

describe('Sidebar', () => {
  it('renders navigation and user profile', () => {
    renderSidebar()

    expect(screen.getByText('Checkgate')).toBeInTheDocument()
    expect(screen.getByText('Dashboard')).toBeInTheDocument()
    expect(screen.getByText('Feature Flags')).toBeInTheDocument()
    expect(screen.getByText('testuser')).toBeInTheDocument()
  })

  it('labels each scope group with the context it applies to', () => {
    renderSidebar()

    // The env group header names the active environment, so it's clear which
    // switcher above changes what these pages show.
    expect(screen.getByRole('button', { name: /environment.*development/i })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /project.*bookspine/i })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /workspace/i })).toBeInTheDocument()
  })

  it('starts with admin sections closed to keep the default list short', async () => {
    const user = userEvent.setup()
    renderSidebar()

    // Environment items are the daily drivers — visible up front.
    expect(screen.getByText('Feature Flags')).toBeInTheDocument()
    // Workspace admin items are tucked away until asked for.
    expect(screen.queryByText('Users')).not.toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: /workspace/i }))

    expect(screen.getByText('Users')).toBeInTheDocument()
  })

  it('keeps a section open when it holds the current page', () => {
    // /users lives in the workspace group, which is closed by default —
    // it must still render, or the active page would be invisible.
    renderSidebar('/users')

    expect(screen.getByText('Users')).toBeInTheDocument()
  })

  it('persists an opened section across remounts', async () => {
    const user = userEvent.setup()
    const { unmount } = renderSidebar()

    await user.click(screen.getByRole('button', { name: /workspace/i }))
    expect(screen.getByText('Users')).toBeInTheDocument()
    unmount()

    renderSidebar()
    expect(screen.getByText('Users')).toBeInTheDocument()
  })
})
