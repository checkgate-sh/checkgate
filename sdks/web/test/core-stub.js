// Stub for the wasm-pack build artifact `./dist/checkgate.js`, aliased in by
// vitest.config.js so the SDK can be imported for unit tests without building
// the WASM. Unit tests white-box their own fake `core` onto the client; the
// fault-injection suite (faults.test.js) connects for real, so the class is a
// small working engine (snake_case, like the wasm-bindgen binding).
import { makeFakeCore } from '../../conformance/fake-core.mjs'

export default async function init() {}

export class CheckgateCoreWasm {
  constructor() { this._core = makeFakeCore() }
  upsert_flag_v2(json) { this._core.upsert(JSON.parse(json)) }
  delete_flag(key) { this._core.delete(key) }
  is_enabled(key, user, attrs) { return this._core.isEnabled(key, user, attrs) }
  get_variant() { return null }
}
