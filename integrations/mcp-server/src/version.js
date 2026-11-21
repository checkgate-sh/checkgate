// The package version, read from package.json so a release bump
// (scripts/bump-version.sh) cannot leave a stale copy in the code.
import { createRequire } from 'node:module'

export const VERSION = createRequire(import.meta.url)('../package.json').version
