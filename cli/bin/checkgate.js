#!/usr/bin/env node
import { run } from '../src/index.js'
import { EXIT } from '../src/errors.js'
import { cleanupPendingWrites } from '../src/write.js'

// Ctrl+C / SIGTERM: drop any half-written temp file and exit with the
// conventional 128+signal code. Target files are never partially written
// (writes are atomic), so there is nothing else to undo.
for (const [signal, code] of [['SIGINT', EXIT.SIGINT], ['SIGTERM', EXIT.SIGTERM]]) {
  process.once(signal, () => {
    cleanupPendingWrites()
    process.stderr.write(`\nInterrupted (${signal}); no output file was left partially written.\n`)
    process.exit(code)
  })
}

run(process.argv.slice(2))
  .then(code => { process.exitCode = code })
  .catch(err => {
    // run() classifies every error itself; reaching here is a bug.
    process.stderr.write(`Error: ${err && err.message ? err.message : err}\n`)
    process.exitCode = EXIT.ERROR
  })
