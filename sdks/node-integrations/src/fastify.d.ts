import type { FastifyPluginAsync, FastifyRequest } from 'fastify'
import type { CheckgateClientOptions } from '@checkgate/node'
import type { CheckgateClientLike, ContextExtractor, RequestFlags, RequestFlagsOptions, StartOptions } from './index'

export type { RequestFlags } from './index'
export { startCheckgate, stopCheckgate } from './index'

declare module 'fastify' {
  interface FastifyInstance {
    /** The shared CheckgateClient. */
    checkgate: CheckgateClientLike
  }
  interface FastifyRequest {
    /** The flags of this request's user. */
    checkgate: RequestFlags
  }
}

interface CommonOptions extends RequestFlagsOptions, Omit<StartOptions, 'logger'> {
  /**
   * Returns the request's `{ key, attributes }`. Called once per request, on
   * the first evaluation. Default: `{ key: request.user.id }`, else anonymous.
   */
  context?: ContextExtractor<FastifyRequest>
  /** Must be below Fastify's `pluginTimeout` (10 s by default). */
  readyTimeoutMs?: number
}

export type CheckgatePluginOptions =
  | (CommonOptions & { client: CheckgateClientLike; clientOptions?: never })
  | (CommonOptions & { clientOptions: CheckgateClientOptions; client?: never })

/**
 * Connects while the plugin loads (so `ready()` / `listen()` wait for it, up
 * to `readyTimeoutMs`) and disconnects in `onClose`.
 */
declare const checkgatePlugin: FastifyPluginAsync<CheckgatePluginOptions>

export default checkgatePlugin
export { checkgatePlugin }
