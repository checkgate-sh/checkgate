#!/usr/bin/env node
import { main } from '../src/index.js'

main().catch(() => {
  // Never print the error object: it could carry request details.
  process.stderr.write('checkgate-mcp: fatal error\n')
  process.exit(1)
})
