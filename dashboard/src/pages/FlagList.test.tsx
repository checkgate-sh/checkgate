import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { MemoryRouter } from 'react-router-dom'
import FlagList from './FlagList'
import { api, scheduledApi, segmentsApi } from '../api'
import type { Flag } from '../types'

const FLAG: Flag = {
  key: 'dark_mode',
  is_enabled: true,
  rollout_percentage: null,
  description: 'Dark theme',
  rules: [],
  flag_type: 'boolean',
  tags: [],
}

vi.mock('../api', () => ({
  api: {
    listFlags: vi.fn(),
    getFlag: vi.fn(),
    createFlag: vi.fn(),
    patchFlag: vi.fn(),
  },
  segmentsApi: { list: vi.fn() },
  scheduledApi: { listForFlag: vi.fn(), create: vi.fn(), delete: vi.fn() },
}))

// The context value must keep a stable identity across renders — the form's
// load effects key off `activeEnv`, so a fresh object each render would
// re-trigger them forever.
const ENV = { id: 'env-1', name: 'Development', color: '#4f46e5' }
const ENV_CTX = {
  environments: [ENV],
  activeEnv: ENV,
  setActiveEnv: vi.fn(),
}

vi.mock('../context/EnvironmentContext', () => ({
  useEnvironment: () => ENV_CTX,
}))

function renderList(initialEntry = '/flags') {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <FlagList />
    </MemoryRouter>,
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(api.listFlags).mockResolvedValue([FLAG])
  vi.mocked(api.getFlag).mockResolvedValue(FLAG)
  vi.mocked(segmentsApi.list).mockResolvedValue([])
  vi.mocked(scheduledApi.listForFlag).mockResolvedValue([])
})

describe('FlagList flag panel', () => {
  it('opens the create panel in a drawer instead of navigating away', async () => {
    const user = userEvent.setup()
    renderList()
    await screen.findByText('dark_mode')

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: /new flag/i }))

    const dialog = await screen.findByRole('dialog', { name: 'New flag' })
    // The key field is editable when creating — this is the create form, not edit.
    expect(within(dialog).getByPlaceholderText('e.g. dark_mode')).toBeEnabled()
  })

  it('opens the edit panel prefilled and with an immutable key', async () => {
    const user = userEvent.setup()
    renderList()
    await screen.findByText('dark_mode')

    await user.click(screen.getByRole('button', { name: /edit flag/i }))

    const dialog = await screen.findByRole('dialog', { name: 'Edit flag' })
    await waitFor(() => {
      expect(within(dialog).getByPlaceholderText('e.g. dark_mode')).toHaveValue('dark_mode')
    })
    // Key is immutable after creation.
    expect(within(dialog).getByPlaceholderText('e.g. dark_mode')).toBeDisabled()
    expect(api.getFlag).toHaveBeenCalledWith('env-1', 'dark_mode')
  })

  it('opens the panel straight from a ?edit= deep link', async () => {
    renderList('/flags?edit=dark_mode')
    expect(await screen.findByRole('dialog', { name: 'Edit flag' })).toBeInTheDocument()
  })

  it('closes the panel on Escape', async () => {
    const user = userEvent.setup()
    renderList('/flags?new=1')
    await screen.findByRole('dialog', { name: 'New flag' })

    await user.keyboard('{Escape}')

    await waitFor(() => {
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    })
  })

  it('adds the created flag to the list and closes the panel', async () => {
    const user = userEvent.setup()
    vi.mocked(api.createFlag).mockResolvedValue({ ...FLAG, key: 'new_flag' })
    renderList('/flags?new=1')

    const dialog = await screen.findByRole('dialog', { name: 'New flag' })
    await user.type(within(dialog).getByPlaceholderText('e.g. dark_mode'), 'new_flag')
    await user.click(within(dialog).getByRole('button', { name: /create flag/i }))

    await waitFor(() => {
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    })
    expect(api.createFlag).toHaveBeenCalledWith('env-1', expect.objectContaining({ key: 'new_flag' }))
    // Merged into the list rather than requiring a manual refresh.
    expect(await screen.findByText('new_flag')).toBeInTheDocument()
  })
})
