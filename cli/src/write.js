// Atomic file writes: a generated file is either the old version or the new
// one, never a truncated mix — even if the process is killed mid-write.

import { mkdir, open, rename, unlink } from 'node:fs/promises'
import { unlinkSync } from 'node:fs'
import { basename, dirname, join } from 'node:path'

/** Temp files currently being written; removed by `cleanupPendingWrites` on a signal. */
const pending = new Set()
let counter = 0

/**
 * Write `data` to `path` by writing a sibling temp file and renaming it over
 * the target (atomic on POSIX within one filesystem; on Windows `rename`
 * replaces the file too). The temp file lives in the target's directory so the
 * rename never crosses filesystems.
 *
 * This protects against the process being interrupted or killed mid-write. It
 * deliberately does not fsync: the output is regenerable source code, and an
 * fsync per file measured ~28 ms each on macOS — more than the whole run.
 */
export async function writeFileAtomic(path, data) {
  const dir = dirname(path)
  await mkdir(dir, { recursive: true })
  // pid + per-process counter is unique among live processes; `wx` below
  // refuses to reuse a leftover from a crashed process with a recycled pid.
  const tmp = join(dir, `.${basename(path)}.${process.pid}.${counter++}.tmp`)
  pending.add(tmp)
  try {
    // `wx`: never follow or clobber something already at the temp path.
    const fh = await open(tmp, 'wx', 0o644)
    try {
      await fh.writeFile(data)
    } finally {
      await fh.close()
    }
    await rename(tmp, path)
  } catch (e) {
    await unlink(tmp).catch(() => {})
    throw e
  } finally {
    pending.delete(tmp)
  }
}

/** Synchronously remove temp files of writes still in progress (signal handlers). */
export function cleanupPendingWrites() {
  for (const tmp of pending) {
    try { unlinkSync(tmp) } catch { /* already gone */ }
  }
  pending.clear()
}
