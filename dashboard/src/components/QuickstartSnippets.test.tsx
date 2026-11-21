import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, it, expect } from 'vitest'
import QuickstartSnippets, { quickstartSnippet } from './QuickstartSnippets'

describe('quickstartSnippet', () => {
  it('embeds the key only when given one; otherwise reads it from the environment', () => {
    const withKey = quickstartSnippet('node', 'http://localhost:3000', 'new-checkout', 'sk_live_abc')
    expect(withKey).toContain("sdkKey: 'sk_live_abc'")
    const without = quickstartSnippet('node', 'http://localhost:3000', 'new-checkout')
    expect(without).toContain('process.env.CHECKGATE_SDK_KEY')
    expect(without).not.toContain('sk_')
    const py = quickstartSnippet('python', 'http://localhost:3000', 'new-checkout')
    expect(py).toContain('os.environ["CHECKGATE_SDK_KEY"]')
    expect(py).toContain('client.is_enabled("new-checkout", "user-123", {"plan": "pro"})')
    expect(quickstartSnippet('python', 'http://x', 'f', 'sk_1')).toContain('CheckgateClient("http://x", "sk_1")')
  })
})

describe('QuickstartSnippets', () => {
  it('switches between Node.js and Python', async () => {
    const user = userEvent.setup()
    render(<QuickstartSnippets />)
    expect(screen.getByTestId('quickstart-snippets')).toHaveTextContent('@checkgate/node')
    await user.click(screen.getByRole('tab', { name: 'Python' }))
    expect(screen.getByTestId('quickstart-snippets')).toHaveTextContent('pip install checkgate')
  })
})
