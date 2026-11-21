import { useState } from 'react'
import { Check, Copy } from 'lucide-react'

// Copyable "install and evaluate" snippets for the first-run experience.
// With `sdkKey` the snippet is ready to paste (shown once, right after setup);
// without it the key is read from CHECKGATE_SDK_KEY, so a key never lands in
// a snippet someone might paste into a repository.

type Lang = 'node' | 'python'

export function quickstartSnippet(lang: Lang, serverUrl: string, flagKey: string, sdkKey?: string): string {
  const key = sdkKey ? `'${sdkKey}'` : null
  if (lang === 'python') {
    return [
      '# pip install checkgate',
      'import os',
      'from checkgate import CheckgateClient',
      '',
      `client = CheckgateClient("${serverUrl}", ${key ? key.replace(/'/g, '"') : 'os.environ["CHECKGATE_SDK_KEY"]'})`,
      'client.connect(timeout=10)',
      `print(client.is_enabled("${flagKey}", "user-123", {"plan": "pro"}))`,
      'client.close()',
    ].join('\n')
  }
  return [
    '// npm install @checkgate/node',
    "const { CheckgateClient } = require('@checkgate/node')",
    '',
    `const client = new CheckgateClient({ serverUrl: '${serverUrl}', sdkKey: ${key ?? 'process.env.CHECKGATE_SDK_KEY'} })`,
    'client.connect().then(() => {',
    `  console.log(client.isEnabled('${flagKey}', 'user-123', { plan: 'pro' }))`,
    '  client.disconnect()',
    '})',
  ].join('\n')
}

export default function QuickstartSnippets({ sdkKey, flagKey = 'new-checkout' }: { sdkKey?: string; flagKey?: string }) {
  const [lang, setLang] = useState<Lang>('node')
  const [copied, setCopied] = useState(false)
  const serverUrl = typeof window === 'undefined' ? 'http://localhost:3000' : window.location.origin
  const code = quickstartSnippet(lang, serverUrl, flagKey, sdkKey)

  async function copy() {
    try {
      await navigator.clipboard.writeText(code)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      // Clipboard unavailable (insecure origin): the text stays selectable.
    }
  }

  return (
    <div className="rounded-xl border border-gray-200 overflow-hidden" data-testid="quickstart-snippets">
      <div className="flex items-center justify-between bg-gray-50 border-b border-gray-200 px-3 py-1.5">
        <div className="flex gap-1" role="tablist">
          {(['node', 'python'] as const).map((l) => (
            <button
              key={l}
              type="button"
              role="tab"
              aria-selected={lang === l}
              onClick={() => setLang(l)}
              className={`px-2.5 py-1 rounded-md text-xs font-semibold ${
                lang === l ? 'bg-white text-gray-900 shadow-sm' : 'text-gray-500 hover:text-gray-700'
              }`}
            >
              {l === 'node' ? 'Node.js' : 'Python'}
            </button>
          ))}
        </div>
        <button
          type="button"
          onClick={() => void copy()}
          className="p-1.5 rounded-md text-gray-400 hover:text-gray-700"
          title="Copy snippet"
          aria-label="Copy snippet"
        >
          {copied ? <Check className="w-4 h-4 text-brand-500" /> : <Copy className="w-4 h-4" />}
        </button>
      </div>
      <pre className="p-3 text-xs font-mono text-gray-800 bg-white overflow-x-auto whitespace-pre">{code}</pre>
    </div>
  )
}
