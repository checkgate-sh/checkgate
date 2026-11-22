// Minimal Express app with Checkgate.
//
//   CHECKGATE_URL=http://localhost:3000 CHECKGATE_SDK_KEY=<your SDK key> node examples/express/server.mjs
//   curl -H 'x-user-id: alice' -H 'x-plan: pro' http://localhost:8080/

import express from 'express'
import { CheckgateClient } from '@checkgate/node'
import { checkgateMiddleware, listenWithCheckgate } from '@checkgate/node-integrations/express'

// One client for the whole process.
const client = new CheckgateClient({
    serverUrl: process.env.CHECKGATE_URL ?? 'http://localhost:3000',
    sdkKey: process.env.CHECKGATE_SDK_KEY,
})

const app = express()

// Stand-in for real authentication, which would set req.user.
app.use((req, _res, next) => {
    const id = req.get('x-user-id')
    if (id) req.user = { id, plan: req.get('x-plan') ?? 'free' }
    next()
})

app.use(checkgateMiddleware({
    client,
    context: (req) => (req.user ? { key: req.user.id, attributes: { plan: req.user.plan } } : null),
}))

app.get('/', (req, res) => {
    res.json({
        user: req.checkgate.context.key,
        newCheckout: req.checkgate.isEnabled('new-checkout'),
        bannerColor: req.checkgate.getValue('banner-color', 'blue'),
    })
})

// Waits up to 5 s for flags, then listens either way (cached or default values
// until the first sync).
const { server, close } = await listenWithCheckgate(app, {
    client,
    port: Number(process.env.PORT ?? 8080),
    readyTimeoutMs: 5000,
    onTimeout: 'continue',
})
console.log(`listening on http://localhost:${server.address().port}`)

// Finish requests in flight, then disconnect from Checkgate.
for (const signal of ['SIGTERM', 'SIGINT']) {
    process.once(signal, () => close().then(() => process.exit(0)))
}
