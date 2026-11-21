'use strict'

// @checkgate/node-integrations: the framework-independent core. The adapters
// are at the subpaths ./express, ./fastify and ./nestjs.

const {
    RequestFlags,
    CheckgateStartupTimeoutError,
    createRequestFlags,
    defaultContext,
    normalizeAttributes,
    normalizeContext,
    startCheckgate,
    stopCheckgate,
} = require('./core.js')
const { createTestClient } = require('./testing.js')

// Plain identifiers in an object literal, so Node's CommonJS export detection
// gives ESM importers every name (`import { startCheckgate } from ...`).
module.exports = {
    RequestFlags,
    CheckgateStartupTimeoutError,
    createRequestFlags,
    defaultContext,
    normalizeAttributes,
    normalizeContext,
    startCheckgate,
    stopCheckgate,
    createTestClient,
}
