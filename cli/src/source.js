// Loads raw flag definitions either from a local JSON file (`--input`) or from a
// running Checkgate server's REST API using a personal access token.

import { readFile } from 'node:fs/promises'
import { CliError, redactUrl, usageError } from './errors.js'

/** Flags per list request — the server's maximum page size. */
const PAGE_SIZE = 500
/** The server clamps deeper offsets, so paging past this would repeat a page. */
const MAX_OFFSET = 1_000_000
/** Default per-request timeout. A hung server must not hang a CI job. */
export const DEFAULT_TIMEOUT_MS = 30_000
/** How much of a server's error body is quoted back to the user. */
const MAX_ERROR_TEXT = 300

/**
 * Fetch the flag list for an environment from the server.
 * Uses Bearer auth (a personal access token, or the environment's SDK key).
 * The server pages the list (200 flags by default), so this follows pages
 * until a short one — one request would leave flags out of the generated types.
 *
 * Every request is bounded by `timeoutMs` (default 30 s); exceeding it is a
 * `timeout` error, distinct from a refused or dropped connection (`network`).
 */
export async function fetchFlagsFromApi({ url, env, token, timeoutMs = DEFAULT_TIMEOUT_MS }) {
  if (!url) throw usageError('Missing server URL (pass --url or set CHECKGATE_URL), or use --input <file>.')
  if (!env) throw usageError('Missing environment id (pass --env or set CHECKGATE_ENV).')
  if (!token) throw usageError('Missing access token (pass --token or set CHECKGATE_TOKEN).')

  let parsed
  try {
    parsed = new URL(url)
  } catch {
    throw usageError(`--url "${redactUrl(url)}" is not a valid URL. Expected something like https://flags.example.com.`)
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    throw usageError(`--url must be http:// or https:// (got ${parsed.protocol}).`)
  }
  if (parsed.username || parsed.password) {
    // fetch() refuses such URLs, and its error repeats the URL verbatim — so
    // the password would land in CI logs. Refuse first, without echoing it.
    throw usageError(`--url must not contain credentials (${redactUrl(url)} had user:password@). Authenticate with --token / CHECKGATE_TOKEN.`)
  }

  const base = url.replace(/\/+$/, '')
  const endpoint = `${base}/api/environments/${encodeURIComponent(env)}/flags`

  const flags = []
  for (let offset = 0; offset <= MAX_OFFSET; offset += PAGE_SIZE) {
    const page = await fetchFlagPage(`${endpoint}?limit=${PAGE_SIZE}&offset=${offset}`, env, token, timeoutMs)
    if (!Array.isArray(page)) {
      throw new CliError('bad_response', `Unexpected response from ${redactUrl(endpoint)}: the flag list was not a JSON array.`, {
        hint: 'Check that --url is the Checkgate server and that it is running the same major version as this CLI.',
      })
    }
    flags.push(...page)
    if (page.length < PAGE_SIZE) break
  }
  return flags
}

/** Read at most MAX_ERROR_TEXT characters of an error body, as one line. */
async function errorText(res) {
  try {
    const text = await res.text()
    let msg = text
    try {
      const body = JSON.parse(text)
      msg = typeof body?.error === 'string' ? body.error : typeof body?.message === 'string' ? body.message : text
    } catch { /* not JSON — quote the text */ }
    msg = msg.replace(/\s+/g, ' ').trim()
    return msg.length > MAX_ERROR_TEXT ? `${msg.slice(0, MAX_ERROR_TEXT)}…` : msg
  } catch {
    return ''
  }
}

async function fetchFlagPage(endpoint, env, token, timeoutMs) {
  const shown = redactUrl(endpoint)
  const signal = AbortSignal.timeout(timeoutMs)
  let res
  try {
    res = await fetch(endpoint, {
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: 'application/json',
      },
      signal,
    })
  } catch (e) {
    throw requestError(e, shown, timeoutMs)
  }

  if (res.status === 401 || res.status === 403) {
    throw new CliError('auth', `Server rejected the token (HTTP ${res.status}) for environment ${env}.`, {
      hint: 'Check --token / CHECKGATE_TOKEN. A personal access token needs access to this environment; an SDK key only works for the environment it belongs to.',
    })
  }
  if (res.status === 404) {
    throw new CliError('not_found', `Environment not found (HTTP 404): ${env}`, {
      hint: 'Check --env / CHECKGATE_ENV — it is the environment id, not its name — and that --url points at the Checkgate server.',
    })
  }
  if (!res.ok) {
    const detail = await errorText(res)
    throw new CliError('server', `Failed to fetch flags from ${shown} (HTTP ${res.status})${detail ? `: ${detail}` : '.'}`, {
      hint: res.status >= 500 ? 'The server failed; retry, or check its logs.' : undefined,
    })
  }

  try {
    return await res.json()
  } catch (e) {
    // A body that is not JSON is a SyntaxError; anything else (the timeout
    // firing mid-body, a dropped connection) is a transport failure.
    if (!(e instanceof SyntaxError)) throw requestError(e, shown, timeoutMs)
    const type = res.headers?.get?.('content-type')
    throw new CliError('bad_response', `${shown} returned ${type ? `${type} content` : 'a response'} that is not JSON.`, {
      hint: 'Is --url the Checkgate server\'s base URL? A dashboard URL, a proxy login page or an SSO redirect returns HTML here.',
    })
  }
}

function requestError(e, shown, timeoutMs) {
  if (e?.name === 'TimeoutError' || e?.name === 'AbortError') {
    return new CliError('timeout', `No response from ${shown} within ${timeoutMs / 1000} s.`, {
      hint: 'Check that the server is reachable from here, or raise --timeout / CHECKGATE_TIMEOUT.',
    })
  }
  const cause = String(e?.cause?.code ?? e?.cause?.message ?? e?.message ?? e)
    .replace(/\/\/[^/@\s]*@/g, '//') // underlying errors may quote the URL
  return new CliError('network', `Could not reach ${shown}: ${cause}`, {
    hint: 'Check --url / CHECKGATE_URL and that the server is running.',
  })
}

/** Read a flag array from a local JSON file (e.g. an exported snapshot). */
export async function readFlagsFromFile(path) {
  let text
  try {
    text = await readFile(path, 'utf8')
  } catch (e) {
    throw new CliError('io', `Could not read --input file "${path}": ${e.message}`)
  }
  try {
    return JSON.parse(text)
  } catch (e) {
    throw new CliError('invalid_input', `--input file "${path}" is not valid JSON: ${e.message}`)
  }
}

/** Resolve the raw flag list from whichever source the options specify. */
export async function loadFlags(opts) {
  if (opts.input) return readFlagsFromFile(opts.input)
  return fetchFlagsFromApi(opts)
}
