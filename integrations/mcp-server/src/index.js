// Entry point: stdio transport only. stdout carries the MCP protocol;
// diagnostics go to stderr, with the token scrubbed from every line.

import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'
import { ConfigError, loadConfig } from './config.js'
import { buildServer } from './server.js'

export { loadConfig, ConfigError } from './config.js'
export { buildServer } from './server.js'

/** Longest wait for in-flight tool calls at shutdown. */
const DRAIN_MS = 2000

function makeLogger(secret) {
  return (msg) => {
    let line = String(msg)
    if (secret) line = line.split(secret).join('[redacted]')
    process.stderr.write(`checkgate-mcp: ${line}\n`)
  }
}

/**
 * Count requests received but not yet answered, at the transport. The
 * protocol layer dispatches a request asynchronously, so a client that
 * writes a request and closes stdin straight away can deliver both before
 * any handler has started; tracking here still sees the request.
 * Returns `idle()`: resolves once every received request has been answered.
 */
function trackRequests(transport) {
  const pending = new Set()
  let waiters = []
  const check = () => {
    if (pending.size > 0) return
    for (const w of waiters) w()
    waiters = []
  }
  const onmessage = transport.onmessage
  transport.onmessage = (msg, extra) => {
    if (msg && msg.method !== undefined && msg.id !== undefined) pending.add(msg.id)
    onmessage?.(msg, extra)
  }
  const send = transport.send.bind(transport)
  transport.send = async (msg, opts) => {
    try {
      await send(msg, opts)
    } finally {
      if (msg && msg.method === undefined && msg.id !== undefined && pending.delete(msg.id)) check()
    }
  }
  return () => (pending.size === 0 ? Promise.resolve() : new Promise((resolve) => { waiters.push(resolve) }))
}

/** Run the stdio server until stdin closes or a termination signal arrives. */
/** True for http:// to anything but a loopback host (the token is sent in clear). */
export function cleartextToRemote(url) {
  const u = new URL(url)
  return u.protocol === 'http:' && !['localhost', '127.0.0.1', '[::1]'].includes(u.hostname)
}

export async function main(env = process.env) {
  let config
  try {
    config = loadConfig(env)
  } catch (e) {
    const log = makeLogger(env.CHECKGATE_TOKEN)
    log(e instanceof ConfigError ? `configuration error: ${e.message}` : 'failed to start')
    process.exitCode = 2
    return
  }
  const log = makeLogger(config.token)
  if (cleartextToRemote(config.url)) {
    log('warning: CHECKGATE_URL is plain http:// to a host that is not this machine; the token travels unencrypted. Use https://.')
  }
  const { server, registered } = buildServer(config, { log })
  const transport = new StdioServerTransport()

  let idle = () => Promise.resolve()
  let closing = false
  const shutdown = async (code) => {
    if (closing) return
    closing = true
    // Let calls already running answer (each is bounded by the request
    // timeout), but never wait longer than DRAIN_MS.
    let timer
    await Promise.race([idle(), new Promise((resolve) => { timer = setTimeout(resolve, DRAIN_MS) })])
    clearTimeout(timer)
    try {
      await server.close()
    } catch { /* already closed */ }
    // stdout to a pipe is asynchronous on macOS: let answers already written
    // reach the client before exiting (bounded, in case the reader is gone).
    await Promise.race([
      new Promise((resolve) => { process.stdout.write('', () => resolve()) }),
      new Promise((resolve) => { setTimeout(resolve, 500).unref() }),
    ])
    process.exit(code)
  }
  // The SDK transport does not watch for end of input; an MCP client that
  // goes away closes our stdin, and we must not linger as an orphan.
  process.stdin.on('end', () => { shutdown(0) })
  process.stdin.on('close', () => { shutdown(0) })
  process.on('SIGTERM', () => { shutdown(0) })
  process.on('SIGINT', () => { shutdown(0) })
  // A closed stdout (client gone) surfaces as EPIPE.
  process.stdout.on('error', () => { shutdown(0) })

  await server.connect(transport)
  idle = trackRequests(transport)
  log(`ready (${registered.length} tools${config.write ? ', write tools enabled' : ', read-only'}; propose guard ${config.requirePropose ? 'on' : 'off'})`)
}
