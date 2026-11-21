// A small HTTP client for the Checkgate management API.
//
// - Bearer auth only; the token is never put in a URL, a message or a log.
// - Every request has one deadline covering connect, headers and body.
// - Response bodies are read incrementally and refused past a byte cap.
// - Redirects are refused: the management API never redirects, and following
//   one could send the token somewhere else.
// - Errors carry a stable `kind` and a message built only from the status
//   code — never from the response body, which may hold user content.

export const USER_AGENT = 'checkgate-mcp/0.2.0'

/** Stable error kinds a tool result reports. */
export const ERROR_KINDS = Object.freeze([
  'auth', // 401
  'forbidden', // 403
  'not_found', // 404
  'conflict', // 409
  'invalid', // 422 (and 400)
  'rate_limited', // 429
  'server', // 5xx
  'http', // any other non-2xx
  'timeout',
  'network',
  'too_large',
  'bad_response',
  'refused', // a local safety guard refused to send the request
  'internal', // a bug in this MCP server
])

export class ApiError extends Error {
  constructor(kind, message, { status, retryAfterSeconds } = {}) {
    super(message)
    this.name = 'ApiError'
    this.kind = kind
    this.status = status
    this.retryAfterSeconds = retryAfterSeconds
  }

  toJSON() {
    const out = { kind: this.kind, message: this.message }
    if (this.status !== undefined) out.status = this.status
    if (this.retryAfterSeconds !== undefined) out.retry_after_seconds = this.retryAfterSeconds
    return out
  }
}

/** Percent-encode one path segment. */
export function seg(value) {
  return encodeURIComponent(String(value))
}

function statusError(status, headers) {
  switch (status) {
    case 401:
      return new ApiError('auth', 'Checkgate rejected the token (HTTP 401). It may be wrong, expired or revoked: check CHECKGATE_TOKEN.', { status })
    case 403:
      return new ApiError(
        'forbidden',
        'Not allowed (HTTP 403). The token\'s scope or its owner\'s project role does not permit this. A "propose" token can only read, propose changes for review, create disabled flags and withdraw its own pending change requests.',
        { status },
      )
    case 404:
      return new ApiError('not_found', 'Not found (HTTP 404): no such project, environment, flag or change request visible to this token.', { status })
    case 409:
      return new ApiError('conflict', 'Conflict (HTTP 409): for example the flag key already exists, or the change request is no longer pending.', { status })
    case 400:
    case 422:
      return new ApiError('invalid', `Checkgate rejected the request as invalid (HTTP ${status}): check the values against the flag schema (rollout 0-100, valid key, known operator, configuration limits).`, { status })
    case 429: {
      const raw = headers?.get?.('retry-after')
      const secs = raw && /^\d{1,6}$/.test(raw.trim()) ? Number(raw.trim()) : undefined
      return new ApiError('rate_limited', `Rate limited by Checkgate (HTTP 429)${secs !== undefined ? `; retry after ${secs} s` : ''}.`, { status, retryAfterSeconds: secs })
    }
    default:
      if (status >= 500) return new ApiError('server', `The Checkgate server failed (HTTP ${status}). Retry later or check its logs.`, { status })
      return new ApiError('http', `Unexpected HTTP ${status} from Checkgate.`, { status })
  }
}

function transportError(e, timeoutMs) {
  if (e instanceof ApiError) return e
  if (e?.name === 'TimeoutError' || e?.name === 'AbortError') {
    return new ApiError('timeout', `No response from the Checkgate server within ${timeoutMs} ms.`)
  }
  // The cause names the failure (ECONNREFUSED, ENOTFOUND, a TLS error). Strip
  // anything URL-shaped with userinfo, just in case, and keep it short.
  const cause = String(e?.cause?.code ?? e?.cause?.message ?? e?.message ?? e)
    .replace(/\/\/[^/@\s]*@/g, '//')
    .slice(0, 200)
  return new ApiError('network', `Could not reach the Checkgate server: ${cause}`)
}

async function readBounded(res, maxBytes) {
  const declared = Number(res.headers?.get?.('content-length'))
  if (Number.isFinite(declared) && declared > maxBytes) {
    try { await res.body?.cancel() } catch { /* ignore */ }
    throw new ApiError('too_large', `The Checkgate response is larger than the ${maxBytes}-byte limit; narrow the request (smaller limit, one environment).`)
  }
  if (!res.body) return ''
  const reader = res.body.getReader()
  const chunks = []
  let total = 0
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    total += value.byteLength
    if (total > maxBytes) {
      try { await reader.cancel() } catch { /* ignore */ }
      throw new ApiError('too_large', `The Checkgate response is larger than the ${maxBytes}-byte limit; narrow the request (smaller limit, one environment).`)
    }
    chunks.push(value)
  }
  return Buffer.concat(chunks.map((c) => Buffer.from(c.buffer, c.byteOffset, c.byteLength))).toString('utf8')
}

/**
 * @param {{ url: string, token: string, timeoutMs: number, maxResponseBytes: number, fetchImpl?: typeof fetch }} cfg
 */
export function createClient(cfg) {
  const { url, token, timeoutMs, maxResponseBytes } = cfg
  const fetchImpl = cfg.fetchImpl ?? globalThis.fetch

  /**
   * @param {string} method
   * @param {string} path  must start with `/api/`; dynamic segments encoded with `seg()`
   * @param {{ query?: Record<string, string|number|boolean|undefined|null>, body?: unknown }} [opts]
   * @returns {Promise<{ status: number, data: any }>}
   */
  async function request(method, path, opts = {}) {
    if (!path.startsWith('/api/')) throw new Error(`internal: bad API path ${path}`)
    const target = new URL(url + path)
    for (const [k, v] of Object.entries(opts.query ?? {})) {
      if (v !== undefined && v !== null && v !== '') target.searchParams.set(k, String(v))
    }
    const headers = {
      Authorization: `Bearer ${token}`,
      Accept: 'application/json',
      'User-Agent': USER_AGENT,
    }
    let body
    if (opts.body !== undefined) {
      headers['Content-Type'] = 'application/json'
      body = JSON.stringify(opts.body)
    }
    const signal = AbortSignal.timeout(timeoutMs)
    let res
    let text
    try {
      res = await fetchImpl(target, { method, headers, body, signal, redirect: 'error' })
      if (res.status >= 300 && res.status < 400) {
        try { await res.body?.cancel() } catch { /* ignore */ }
        throw new ApiError('bad_response', `Checkgate answered with a redirect (HTTP ${res.status}); refusing to follow it with the token. Check CHECKGATE_URL.`, { status: res.status })
      }
      if (!res.ok) {
        try { await res.body?.cancel() } catch { /* ignore */ }
        throw statusError(res.status, res.headers)
      }
      text = await readBounded(res, maxResponseBytes)
    } catch (e) {
      throw transportError(e, timeoutMs)
    }
    if (text === '') return { status: res.status, data: null }
    try {
      return { status: res.status, data: JSON.parse(text) }
    } catch {
      throw new ApiError('bad_response', `Checkgate returned a response that is not JSON (HTTP ${res.status}). Is CHECKGATE_URL the server's base URL rather than a proxy or login page?`, { status: res.status })
    }
  }

  return { request }
}
