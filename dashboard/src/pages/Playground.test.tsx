import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import Playground, { exampleContext, parseAttributes } from './Playground'
import { api } from '../api'
import type { Flag, PlaygroundEvaluation } from '../types'

vi.mock('../api', () => ({
  api: { listFlags: vi.fn(), evaluateFlag: vi.fn() },
}))

const ENV = { id: 'env-1', name: 'Development', color: '#4f46e5' }
const ENV_CTX = { environments: [ENV], activeEnv: ENV, setActiveEnv: vi.fn() }
vi.mock('../context/EnvironmentContext', () => ({ useEnvironment: () => ENV_CTX }))

const FLAG: Flag = {
  key: 'new-checkout',
  is_enabled: true,
  rollout_percentage: 30,
  description: null,
  rules: [
    { attribute: 'plan', operator: 'equals', values: ['pro', 'team'] },
    { attribute: 'country', operator: 'equals', values: ['PH'] },
  ],
  flag_type: 'boolean',
}

const RESULT: PlaygroundEvaluation = {
  environment_id: 'env-1',
  flag_key: 'new-checkout',
  flag_type: 'boolean',
  revision: 17,
  enabled: true,
  value: true,
  reason: 'TARGETING_MATCH',
  matched_rule: {
    rule: { attribute: 'plan', operator: 'equals', values: ['pro', 'team'] },
    source_index: 0,
  },
  prerequisites: [],
  referenced_attributes: ['country', 'plan'],
  missing_attributes: ['country'],
}

describe('parseAttributes', () => {
  it('accepts scalars and an empty editor', () => {
    expect(parseAttributes('')).toEqual({ attributes: {} })
    expect(parseAttributes('{"plan":"pro","age":30,"beta":true}')).toEqual({
      attributes: { plan: 'pro', age: 30, beta: true },
    })
  })
  it('refuses invalid JSON, non-objects, nested values and too many attributes', () => {
    for (const bad of ['{', '[1]', '"x"', 'null', '{"o":{"a":1}}', '{"n":null}', '{"l":[1]}']) {
      expect(parseAttributes(bad).error, bad).toBeTruthy()
    }
    const many = JSON.stringify(Object.fromEntries(Array.from({ length: 101 }, (_, i) => [`a${i}`, 'v'])))
    expect(parseAttributes(many).error).toMatch(/At most 100/)
  })
})

describe('exampleContext', () => {
  it('uses the first value of each rule attribute', () => {
    expect(exampleContext(FLAG)).toEqual({ plan: 'pro', country: 'PH' })
    expect(exampleContext(undefined)).toEqual({})
  })
})

describe('Playground', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(api.listFlags).mockResolvedValue([FLAG])
    vi.mocked(api.evaluateFlag).mockResolvedValue(RESULT)
  })

  it('evaluates on the server and explains the result', async () => {
    const user = userEvent.setup()
    render(<Playground />)
    await waitFor(() => expect(screen.getByLabelText('Flag')).toHaveValue('new-checkout'))
    await user.click(screen.getByRole('button', { name: /Example from this flag/ }))
    await user.click(screen.getByRole('button', { name: /Evaluate/ }))
    expect(api.evaluateFlag).toHaveBeenCalledWith('env-1', 'new-checkout', 'user-123', { plan: 'pro', country: 'PH' })
    expect(await screen.findByTestId('result-reason')).toHaveTextContent('TARGETING_MATCH')
    expect(screen.getByTestId('result-state')).toHaveTextContent('ENABLED')
    expect(screen.getByText(/#1: plan equals pro, team/)).toBeInTheDocument()
    expect(screen.getByText('17')).toBeInTheDocument()
  })

  it('blocks evaluation while the attributes are invalid', async () => {
    const user = userEvent.setup()
    render(<Playground />)
    await waitFor(() => expect(screen.getByLabelText('Flag')).toHaveValue('new-checkout'))
    const editor = screen.getByLabelText('Attributes')
    await user.clear(editor)
    await user.type(editor, '{{"o": [1]}')
    expect(screen.getByRole('alert')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Evaluate/ })).toBeDisabled()
    expect(api.evaluateFlag).not.toHaveBeenCalled()
  })

  it('shows server errors and renders untrusted text as text', async () => {
    vi.mocked(api.evaluateFlag).mockRejectedValue(new Error('<img src=x onerror=alert(1)>'))
    const user = userEvent.setup()
    const { container } = render(<Playground />)
    await waitFor(() => expect(screen.getByLabelText('Flag')).toHaveValue('new-checkout'))
    await user.click(screen.getByRole('button', { name: /Evaluate/ }))
    expect(await screen.findByRole('alert')).toHaveTextContent('<img src=x onerror=alert(1)>')
    expect(container.querySelector('img')).toBeNull()
  })
})
