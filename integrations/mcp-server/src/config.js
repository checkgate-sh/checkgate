// Configuration from environment variables. Every error message here is safe
// to print: it never contains the token, and never repeats a URL that had
// credentials in it.

/** Default per-request timeout (whole request, body included). */
export const DEFAULT_TIMEOUT_MS = 10_000
const MIN_TIMEOUT_MS = 100
const MAX_TIMEOUT_MS = 120_000

/** Default cap on one HTTP response body. */
export const DEFAULT_MAX_RESPONSE_BYTES = 2 * 1024 * 1024
const MIN_MAX_RESPONSE_BYTES = 1024
const MAX_MAX_RESPONSE_BYTES = 32 * 1024 * 1024

export class ConfigError extends Error {
  constructor(message) {
    super(message)
    this.name = 'ConfigError'
  }
}

/**
 * Remove credentials, query and fragment from a URL before it is shown
 * (`https://user:secret@host/?t=x` → `https://host`).
 */
export function redactUrl(raw) {
  try {
    const u = new URL(String(raw))
    u.username = ''
    u.password = ''
    u.search = ''
    u.hash = ''
    return u.toString().replace(/\/$/, '')
  } catch {
    return '<unparseable URL>'
  }
}

function flag(value, defaultValue) {
  if (value === undefined || value === '') return defaultValue
  const v = String(value).trim().toLowerCase()
  if (v === '1' || v === 'true' || v === 'yes') return true
  if (v === '0' || v === 'false' || v === 'no') return false
  return null
}

function boundedInt(name, value, min, max, defaultValue) {
  if (value === undefined || value === '') return defaultValue
  if (!/^\d+$/.test(String(value).trim())) {
    throw new ConfigError(`${name} must be a whole number between ${min} and ${max}.`)
  }
  const n = Number(String(value).trim())
  if (!Number.isSafeInteger(n) || n < min || n > max) {
    throw new ConfigError(`${name} must be a whole number between ${min} and ${max}.`)
  }
  return n
}

/**
 * Parse and validate the configuration.
 *
 * - `CHECKGATE_URL` (required): http(s) base URL of the Checkgate server.
 *   Refused if it carries `user:password@`, a query string or a fragment.
 * - `CHECKGATE_TOKEN` (required): a personal access token. A `propose`-scoped
 *   token is strongly recommended.
 * - `CHECKGATE_MCP_WRITE=1`: register the mutation tools at all (default: off).
 * - `CHECKGATE_MCP_REQUIRE_PROPOSE` (default on): refuse to send a flag PATCH
 *   unless the token is a `propose` token or the environment requires
 *   approval (and the analogous checks for the other write tools).
 * - `CHECKGATE_TIMEOUT_MS` (default 10000), `CHECKGATE_MCP_MAX_RESPONSE_BYTES`
 *   (default 2 MiB).
 */
export function loadConfig(env = process.env) {
  const rawUrl = env.CHECKGATE_URL
  if (!rawUrl) throw new ConfigError('CHECKGATE_URL is not set. Set it to your Checkgate server, e.g. https://flags.example.com.')
  let parsed
  try {
    parsed = new URL(rawUrl)
  } catch {
    throw new ConfigError('CHECKGATE_URL is not a valid URL. Expected something like https://flags.example.com.')
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    throw new ConfigError(`CHECKGATE_URL must be http:// or https:// (got ${parsed.protocol}).`)
  }
  if (parsed.username || parsed.password) {
    // Never echo it: the password would land in the MCP client's logs.
    throw new ConfigError(`CHECKGATE_URL must not contain credentials (${redactUrl(rawUrl)} had user:password@). Put the token in CHECKGATE_TOKEN.`)
  }
  if (parsed.search || parsed.hash) {
    throw new ConfigError(`CHECKGATE_URL must be a plain base URL without a query string or fragment (${redactUrl(rawUrl)}).`)
  }
  const url = parsed.toString().replace(/\/+$/, '')

  const token = env.CHECKGATE_TOKEN
  if (!token) throw new ConfigError('CHECKGATE_TOKEN is not set. Create a personal access token (scope "propose" recommended) under Settings → Access Tokens.')
  // Header-safe printable ASCII only; also stops a pasted newline from
  // turning into a header-injection or a confusing fetch error.
  if (token.length > 512 || !/^[\x21-\x7e]+$/.test(token)) {
    throw new ConfigError('CHECKGATE_TOKEN is malformed (expected a single printable token without spaces).')
  }

  const write = flag(env.CHECKGATE_MCP_WRITE, false)
  if (write === null) throw new ConfigError('CHECKGATE_MCP_WRITE must be 1 or 0.')
  const requirePropose = flag(env.CHECKGATE_MCP_REQUIRE_PROPOSE, true)
  if (requirePropose === null) throw new ConfigError('CHECKGATE_MCP_REQUIRE_PROPOSE must be 1 or 0.')

  return Object.freeze({
    url,
    token,
    write,
    requirePropose,
    timeoutMs: boundedInt('CHECKGATE_TIMEOUT_MS', env.CHECKGATE_TIMEOUT_MS, MIN_TIMEOUT_MS, MAX_TIMEOUT_MS, DEFAULT_TIMEOUT_MS),
    maxResponseBytes: boundedInt(
      'CHECKGATE_MCP_MAX_RESPONSE_BYTES',
      env.CHECKGATE_MCP_MAX_RESPONSE_BYTES,
      MIN_MAX_RESPONSE_BYTES,
      MAX_MAX_RESPONSE_BYTES,
      DEFAULT_MAX_RESPONSE_BYTES,
    ),
  })
}
