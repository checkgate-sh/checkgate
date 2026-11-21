'use strict'

// @checkgate/node-integrations: the framework-independent core. The adapters
// are at the subpaths ./express, ./fastify and ./nestjs.

const core = require('./core.js')
const { createTestClient } = require('./testing.js')

module.exports = {
    RequestFlags: core.RequestFlags,
    CheckgateStartupTimeoutError: core.CheckgateStartupTimeoutError,
    createRequestFlags: core.createRequestFlags,
    defaultContext: core.defaultContext,
    normalizeAttributes: core.normalizeAttributes,
    normalizeContext: core.normalizeContext,
    startCheckgate: core.startCheckgate,
    stopCheckgate: core.stopCheckgate,
    createTestClient,
}
