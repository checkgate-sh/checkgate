// Error classification and exit codes for the `checkgate` CLI.
//
// Exit codes are part of the CLI's contract (see README "Exit codes"):
//   0  success — files written, or `--check` found everything up to date
//   1  operational failure — network, timeout, auth, server, bad data, file I/O
//   2  usage error — unknown/missing/conflicting options (nothing was attempted)
//   3  `--check` found generated files that are missing or out of date
// Ctrl+C / SIGTERM end the process with the conventional 130 / 143.

export const EXIT = Object.freeze({
  OK: 0,
  ERROR: 1,
  USAGE: 2,
  OUT_OF_DATE: 3,
  SIGINT: 130,
  SIGTERM: 143,
})

/**
 * Stable machine-readable error codes (the `error.code` field of `--json`
 * output). Adding a code is a compatible change; renaming one is not.
 */
export const ERROR_CODES = Object.freeze([
  'usage',          // invalid invocation — exit 2
  'invalid_input',  // flag data that cannot be turned into valid code
  'auth',           // the server rejected the token (401/403)
  'not_found',      // environment (or endpoint) not found (404)
  'network',        // could not connect / connection dropped
  'timeout',        // the server did not answer within --timeout
  'server',         // the server answered with another non-2xx status
  'bad_response',   // a 2xx response that is not the expected JSON
  'io',             // reading --input or writing output failed
  'internal',       // a bug in the CLI
])

/** An expected, classified failure. Anything else reaching the top is `internal`. */
export class CliError extends Error {
  /**
   * @param {string} code one of ERROR_CODES
   * @param {string} message what failed, and why
   * @param {{ hint?: string, findings?: object[] }} [extra]
   */
  constructor(code, message, extra = {}) {
    super(message)
    this.name = 'CliError'
    this.code = code
    this.hint = extra.hint
    this.findings = extra.findings ?? []
  }

  get exitCode() {
    return this.code === 'usage' ? EXIT.USAGE : EXIT.ERROR
  }
}

export function usageError(message, hint) {
  return new CliError('usage', message, { hint: hint ?? 'Run `checkgate --help` for usage.' })
}

/**
 * Remove credentials from a URL before it is printed or written into generated
 * files (`https://user:secret@host` → `https://host`). Query strings are also
 * dropped: they are the other place people put tokens.
 */
export function redactUrl(raw) {
  try {
    const u = new URL(String(raw))
    u.username = ''
    u.password = ''
    u.search = ''
    u.hash = ''
    return u.toString().replace(/\/$/, '')
  } catch {
    // Not a parseable URL: strip anything that looks like `userinfo@`.
    return String(raw).replace(/\/\/[^/@\s]*@/, '//')
  }
}
