import { test, afterEach } from 'node:test'
import assert from 'node:assert/strict'
import { fetchFlagsFromApi } from '../src/source.js'

const realFetch = globalThis.fetch
afterEach(() => { globalThis.fetch = realFetch })

test('fetchFlagsFromApi follows pages until a short one', async () => {
  const all = Array.from({ length: 711 }, (_, i) => ({ key: `flag-${i}`, is_enabled: true }))
  const urls = []
  globalThis.fetch = async (url) => {
    urls.push(url)
    const params = new URL(url).searchParams
    const offset = Number(params.get('offset'))
    const page = all.slice(offset, offset + Number(params.get('limit')))
    return { ok: true, status: 200, json: async () => page }
  }

  const flags = await fetchFlagsFromApi({ url: 'http://cg.test/', env: 'env-1', token: 'pat' })

  assert.deepEqual(flags, all)
  assert.deepEqual(urls, [
    'http://cg.test/api/environments/env-1/flags?limit=500&offset=0',
    'http://cg.test/api/environments/env-1/flags?limit=500&offset=500',
  ])
})

test('fetchFlagsFromApi surfaces an auth failure on any page', async () => {
  globalThis.fetch = async () => ({ ok: false, status: 403, json: async () => null })
  await assert.rejects(
    fetchFlagsFromApi({ url: 'http://cg.test', env: 'env-1', token: 'pat' }),
    /rejected the token \(HTTP 403\)/,
  )
})
