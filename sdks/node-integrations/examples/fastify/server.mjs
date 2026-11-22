// Minimal Fastify app with Checkgate.
//
//   CHECKGATE_URL=http://localhost:3000 CHECKGATE_SDK_KEY=<your SDK key> node examples/fastify/server.mjs
//   curl -H 'x-user-id: alice' -H 'x-plan: pro' http://localhost:8080/

import Fastify from 'fastify'
import { checkgatePlugin } from '@checkgate/node-integrations/fastify'

const app = Fastify({ logger: { level: 'warn' } })

// Stand-in for real authentication, which would set request.user.
app.decorateRequest('user', null)
app.addHook('onRequest', async (request) => {
    const id = request.headers['x-user-id']
    if (id) request.user = { id, plan: request.headers['x-plan'] ?? 'free' }
})

// The plugin builds the client, connects while loading (up to 5 s; then the
// app starts with cached or default values) and disconnects on close.
await app.register(checkgatePlugin, {
    clientOptions: {
        serverUrl: process.env.CHECKGATE_URL ?? 'http://localhost:3000',
        sdkKey: process.env.CHECKGATE_SDK_KEY,
    },
    context: (request) => (request.user ? { key: request.user.id, attributes: { plan: request.user.plan } } : null),
    readyTimeoutMs: 5000,
    onTimeout: 'continue',
})

app.get('/', async (request) => ({
    user: request.checkgate.context.key,
    newCheckout: request.checkgate.isEnabled('new-checkout'),
    bannerColor: request.checkgate.getValue('banner-color', 'blue'),
}))

const address = await app.listen({ port: Number(process.env.PORT ?? 8080), host: '127.0.0.1' })
console.log(`listening on ${address}`)

for (const signal of ['SIGTERM', 'SIGINT']) {
    process.once(signal, () => app.close().then(() => process.exit(0)))
}
