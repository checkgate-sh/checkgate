// Connects an in-process MCP client to the server, over the SDK's in-memory
// transport, against a fake Checkgate API.

import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js'
import { loadConfig } from '../src/config.js'
import { buildServer } from '../src/server.js'
import { DATA_CLOSE, DATA_OPEN } from '../src/result.js'
import { TOKENS, startFakeApi } from './fake-api.js'

export async function connect({ scope = 'propose', write = false, env = {} } = {}) {
  const fake = await startFakeApi()
  const config = loadConfig({
    CHECKGATE_URL: fake.url,
    CHECKGATE_TOKEN: TOKENS[scope] ?? scope,
    CHECKGATE_MCP_WRITE: write ? '1' : '0',
    ...env,
  })
  const logs = []
  const { server } = buildServer(config, { log: (m) => logs.push(m) })
  const [clientSide, serverSide] = InMemoryTransport.createLinkedPair()
  await server.connect(serverSide)
  const client = new Client({ name: 'test', version: '1.0.0' })
  await client.connect(clientSide)
  return {
    client,
    fake,
    logs,
    config,
    async call(name, args = {}) {
      return client.callTool({ name, arguments: args })
    },
    async close() {
      await client.close()
      await server.close()
      await fake.close()
    },
  }
}

/** The JSON inside the delimited data block of a text result. */
export function dataBlock(result) {
  const text = result.content[0].text
  const start = text.indexOf(DATA_OPEN)
  const end = text.lastIndexOf(DATA_CLOSE)
  if (start < 0 || end < 0) throw new Error(`no data block in: ${text}`)
  return JSON.parse(text.slice(start + DATA_OPEN.length, end))
}
