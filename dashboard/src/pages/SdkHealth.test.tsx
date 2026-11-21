import { render, screen } from '@testing-library/react'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import SdkHealth from './SdkHealth'
import { healthApi } from '../api'

vi.mock('../api', () => ({ healthApi: { connections: vi.fn(), environments: vi.fn() } }))
const ENVS = [{ id: 'env-prod', name: 'Production', color: '#000' }]
const ENV_CTX = { environments: ENVS, activeEnv: ENVS[0], setActiveEnv: vi.fn() }
vi.mock('../context/EnvironmentContext', () => ({ useEnvironment: () => ENV_CTX }))

describe('SdkHealth', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    const now = Math.floor(Date.now() / 1000)
    vi.mocked(healthApi.connections).mockResolvedValue([
      { connection_id: 'c1', environment_id: 'env-prod', sdk_key_name: 'api', client_ip: '10.0.0.0/24', connected_at: now - 30, last_delivered_revision: 41 },
      { connection_id: 'c2', environment_id: 'env-prod', sdk_key_name: 'api', client_ip: '10.0.0.0/24', connected_at: now - 5, last_delivered_revision: 42 },
    ])
    vi.mocked(healthApi.environments).mockResolvedValue([
      { environment_id: 'env-prod', connections: 2, latest_revision: 42, connections_behind: 1, oldest_connected_at: now - 30 },
    ])
  })

  it('shows per-environment convergence and delivered revisions, labelled as server-observed', async () => {
    render(<SdkHealth />)
    expect(await screen.findByText('1 behind')).toBeInTheDocument()
    expect(screen.getByTestId('env-health')).toHaveTextContent('Production')
    expect(screen.getByTestId('env-health')).toHaveTextContent('latest revision 42')
    expect(screen.getByText('41')).toBeInTheDocument()
    expect(screen.getByText(/server observes/)).toBeInTheDocument()
  })
})
