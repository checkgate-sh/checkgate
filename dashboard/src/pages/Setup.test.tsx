import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { MemoryRouter } from 'react-router-dom'
import Setup from './Setup'

const completeSetup = vi.fn()

vi.mock('../context/AuthContext', () => ({
  useAuth: () => ({ completeSetup }),
}))

const fetchMock = vi.fn()

function renderSetup() {
  return render(
    <MemoryRouter>
      <Setup />
    </MemoryRouter>,
  )
}

function jsonResponse(status: number, body: unknown = {}) {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('Setup token step', () => {
  it('sends the token in a header, never the URL, and advances when it is valid', async () => {
    fetchMock.mockResolvedValue(jsonResponse(200, { key: 'sk_live_abc' }))
    const user = userEvent.setup()
    renderSetup()

    const start = screen.getByRole('button', { name: /get started/i })
    expect(start).toBeDisabled()
    await user.type(screen.getByLabelText(/setup token/i), 'tok-0123456789abcdef')
    await user.click(start)

    expect(await screen.findByText(/name your workspace/i)).toBeInTheDocument()
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(url).toBe('/api/setup/key')
    expect(url).not.toContain('tok-')
    expect(init.headers).toEqual({ 'X-Checkgate-Setup-Token': 'tok-0123456789abcdef' })
  })

  it('stays on the first step and explains an invalid token', async () => {
    fetchMock.mockResolvedValue(jsonResponse(401))
    const user = userEvent.setup()
    renderSetup()

    await user.type(screen.getByLabelText(/setup token/i), 'wrong')
    await user.click(screen.getByRole('button', { name: /get started/i }))

    expect(await screen.findByRole('alert')).toHaveTextContent(/not valid/i)
    expect(screen.queryByText(/name your workspace/i)).not.toBeInTheDocument()
  })

  it('points to sign-in when setup is already complete', async () => {
    fetchMock.mockResolvedValue(jsonResponse(404))
    const user = userEvent.setup()
    renderSetup()

    await user.type(screen.getByLabelText(/setup token/i), 'whatever')
    await user.click(screen.getByRole('button', { name: /get started/i }))

    expect(await screen.findByRole('alert')).toHaveTextContent(/already complete/i)
  })
})
