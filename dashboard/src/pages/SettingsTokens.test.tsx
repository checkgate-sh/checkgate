import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { PersonalAccessTokensSection, scopeLabel } from './Settings'
import { tokensApi, type TokenInfo } from '../api'

vi.mock('../api', () => ({
  tokensApi: { list: vi.fn(), create: vi.fn(), revoke: vi.fn() },
}))

const AGENT: TokenInfo = {
  id: 1,
  name: 'Claude',
  prefix: 'pat_0123abcd…',
  scope: 'propose',
  created_at: '2026-11-21T00:00:00Z',
  last_used_at: null,
  expires_at: null,
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(tokensApi.list).mockResolvedValue([AGENT])
  vi.mocked(tokensApi.create).mockResolvedValue({ ...AGENT, id: 2, token: 'pat_x' })
})

describe('personal access token scopes', () => {
  it('labels every scope', () => {
    expect(scopeLabel('read_only')).toBe('read-only')
    expect(scopeLabel('propose')).toBe('propose-only')
    expect(scopeLabel('read_write')).toBe('read-write')
  })

  it('lists a propose token and can create one, explaining what it may do', async () => {
    const user = userEvent.setup()
    render(<PersonalAccessTokensSection />)
    expect(await screen.findByText(/propose-only/)).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: /generate new token/i }))
    expect(screen.queryByText(/someone else must approve/)).not.toBeInTheDocument()
    await user.type(screen.getByPlaceholderText(/token name/i), 'Claude')
    const [scope] = screen.getAllByRole('combobox')
    await user.selectOptions(scope, 'propose')
    expect(screen.getByText(/someone else must approve/)).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Create' }))
    expect(tokensApi.create).toHaveBeenCalledWith('Claude', 'propose', 90)
  })
})
