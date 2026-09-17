import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import Integrations, { eventLabel } from './Integrations'
import { integrationsApi } from '../api'
import type { Integration } from '../types'

const SLACK: Integration = {
  id: 'int-1',
  environment_id: 'env-1',
  kind: 'slack',
  name: '#engineering',
  webhook_url_preview: '…bXXXX',
  events: [],
  enabled: true,
  created_at: '2026-07-18T00:00:00Z',
}

vi.mock('../api', () => ({
  integrationsApi: {
    list: vi.fn(),
    create: vi.fn(),
    patch: vi.fn(),
    delete: vi.fn(),
    test: vi.fn(),
    listDeliveries: vi.fn(),
  },
}))

const ENV = { id: 'env-1', name: 'Production', color: '#ef4444' }
const ENV_CTX = { environments: [ENV], activeEnv: ENV, setActiveEnv: vi.fn() }

vi.mock('../context/EnvironmentContext', () => ({
  useEnvironment: () => ENV_CTX,
}))

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(integrationsApi.list).mockResolvedValue([SLACK])
})

describe('eventLabel', () => {
  it('renders dotted event names as readable text', () => {
    expect(eventLabel('flag.created')).toBe('Flag created')
    expect(eventLabel('change_request.opened')).toBe('Change request opened')
  })
})

describe('Integrations page', () => {
  it('lists connected channels without exposing the webhook URL', async () => {
    render(<Integrations />)

    expect(await screen.findByText('#engineering')).toBeInTheDocument()
    // Only the elided preview is ever rendered — the real URL is a credential
    // and the server never returns it.
    expect(screen.getByText(/…bXXXX/)).toBeInTheDocument()
    expect(screen.queryByText(/hooks\.slack\.com/)).not.toBeInTheDocument()
  })

  it('shows "All events" when no filter is set', async () => {
    render(<Integrations />)
    expect(await screen.findByText('All events')).toBeInTheDocument()
  })

  it('lists the selected events when a filter is set', async () => {
    vi.mocked(integrationsApi.list).mockResolvedValue([
      { ...SLACK, events: ['flag.deleted', 'change_request.opened'] },
    ])
    render(<Integrations />)
    await screen.findByText('#engineering')

    // Scoped to the table: these labels also exist as selectable chips in the
    // create form above.
    const row = screen.getByRole('row', { name: /#engineering/ })
    expect(within(row).getByText('Flag deleted')).toBeInTheDocument()
    expect(within(row).getByText('Change request opened')).toBeInTheDocument()
    expect(screen.queryByText('All events')).not.toBeInTheDocument()
  })

  it('creates an integration with the chosen provider and events', async () => {
    const user = userEvent.setup()
    vi.mocked(integrationsApi.create).mockResolvedValue({
      ...SLACK,
      id: 'int-2',
      name: '#ops',
    })
    render(<Integrations />)
    await screen.findByText('#engineering')

    await user.click(screen.getByRole('button', { name: 'Microsoft Teams' }))
    await user.type(screen.getByPlaceholderText('e.g. #engineering'), '#ops')
    await user.type(
      screen.getByPlaceholderText(/hooks\.slack\.com/),
      'https://outlook.office.com/webhook/abc',
    )
    // Pick a single event so the filter is exercised, not just the default.
    const form = screen.getByText('Connect a channel').closest('form')!
    await user.click(within(form).getByRole('button', { name: 'Flag deleted' }))
    await user.click(screen.getByRole('button', { name: 'Connect' }))

    await waitFor(() => {
      expect(integrationsApi.create).toHaveBeenCalledWith('env-1', {
        kind: 'teams',
        name: '#ops',
        webhook_url: 'https://outlook.office.com/webhook/abc',
        events: ['flag.deleted'],
      })
    })
  })

  it('surfaces a server rejection instead of failing silently', async () => {
    const user = userEvent.setup()
    vi.mocked(integrationsApi.create).mockRejectedValue(new Error('422 Unprocessable Entity'))
    render(<Integrations />)
    await screen.findByText('#engineering')

    await user.type(screen.getByPlaceholderText('e.g. #engineering'), 'bad')
    await user.type(screen.getByPlaceholderText(/hooks\.slack\.com/), 'https://x.test/h')
    await user.click(screen.getByRole('button', { name: 'Connect' }))

    expect(await screen.findByText(/422 Unprocessable Entity/)).toBeInTheDocument()
  })

  it('toggles enabled state through the API', async () => {
    const user = userEvent.setup()
    vi.mocked(integrationsApi.patch).mockResolvedValue({ ...SLACK, enabled: false })
    render(<Integrations />)
    await screen.findByText('#engineering')

    await user.click(screen.getByRole('button', { name: 'Disable integration' }))

    await waitFor(() => {
      expect(integrationsApi.patch).toHaveBeenCalledWith('env-1', 'int-1', { enabled: false })
    })
    expect(await screen.findByRole('button', { name: 'Enable integration' })).toBeInTheDocument()
  })

  it('sends a test message', async () => {
    const user = userEvent.setup()
    vi.mocked(integrationsApi.test).mockResolvedValue(undefined)
    render(<Integrations />)
    await screen.findByText('#engineering')

    await user.click(screen.getByRole('button', { name: 'Send test message' }))

    await waitFor(() => {
      expect(integrationsApi.test).toHaveBeenCalledWith('env-1', 'int-1')
    })
  })

  it('removes a row after a confirmed disconnect', async () => {
    const user = userEvent.setup()
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    vi.mocked(integrationsApi.delete).mockResolvedValue(undefined)
    render(<Integrations />)
    await screen.findByText('#engineering')

    await user.click(screen.getByRole('button', { name: 'Disconnect integration' }))

    await waitFor(() => {
      expect(screen.queryByText('#engineering')).not.toBeInTheDocument()
    })
  })

  it('keeps the row when the disconnect is cancelled', async () => {
    const user = userEvent.setup()
    vi.spyOn(window, 'confirm').mockReturnValue(false)
    render(<Integrations />)
    await screen.findByText('#engineering')

    await user.click(screen.getByRole('button', { name: 'Disconnect integration' }))

    expect(integrationsApi.delete).not.toHaveBeenCalled()
    expect(screen.getByText('#engineering')).toBeInTheDocument()
  })

  it('offers a retry when loading fails', async () => {
    const user = userEvent.setup()
    vi.mocked(integrationsApi.list).mockRejectedValueOnce(new Error('boom'))
    render(<Integrations />)

    expect(await screen.findByText('boom')).toBeInTheDocument()

    vi.mocked(integrationsApi.list).mockResolvedValue([SLACK])
    await user.click(screen.getByRole('button', { name: /retry/i }))

    expect(await screen.findByText('#engineering')).toBeInTheDocument()
  })

  it('shows an empty state when nothing is connected', async () => {
    vi.mocked(integrationsApi.list).mockResolvedValue([])
    render(<Integrations />)
    expect(await screen.findByText('No channels connected yet.')).toBeInTheDocument()
  })

  it('explains that an empty event selection means all events', async () => {
    render(<Integrations />)
    const form = (await screen.findByText('Connect a channel')).closest('form')!
    expect(
      within(form).getByText('Nothing selected — every event will be sent.'),
    ).toBeInTheDocument()
  })
})
