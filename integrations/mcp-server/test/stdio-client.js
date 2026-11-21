// Spawns the real MCP server process and speaks newline-delimited JSON-RPC
// to it over stdio (the transport MCP clients such as Claude use).

import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'

export const BIN = fileURLToPath(new URL('../bin/checkgate-mcp.js', import.meta.url))

export function start(env) {
  const child = spawn(process.execPath, [BIN], {
    env: { PATH: process.env.PATH, ...env },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  let out = ''
  let err = ''
  const waiting = new Map()
  child.stdout.setEncoding('utf8')
  child.stderr.setEncoding('utf8')
  child.stdout.on('data', (d) => {
    out += d
    let nl
    while ((nl = out.indexOf('\n')) >= 0) {
      const line = out.slice(0, nl)
      out = out.slice(nl + 1)
      if (!line.trim()) continue
      const msg = JSON.parse(line)
      waiting.get(msg.id)?.(msg)
      waiting.delete(msg.id)
    }
  })
  child.stderr.on('data', (d) => { err += d })
  const exited = new Promise((resolve) => child.on('exit', (code, signal) => resolve({ code, signal })))
  let nextId = 1
  return {
    child,
    exited,
    stderr: () => err,
    send(method, params) {
      const id = nextId++
      child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`)
      return new Promise((resolve, reject) => {
        const t = setTimeout(() => reject(new Error(`no answer to ${method}; stderr: ${err}`)), 30_000)
        waiting.set(id, (m) => { clearTimeout(t); resolve(m) })
      })
    },
    notify(method, params) {
      child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', method, params })}\n`)
    },
  }
}

export async function initialize(p) {
  const init = await p.send('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'stdio-test', version: '1' } })
  assert.equal(init.result.serverInfo.name, 'checkgate')
  p.notify('notifications/initialized', {})
  return init
}

export function within(promise, ms, what) {
  return Promise.race([promise, new Promise((_, reject) => setTimeout(() => reject(new Error(`${what} took over ${ms} ms`)), ms))])
}

