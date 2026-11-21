import type { Server, IncomingMessage, RequestListener } from 'node:http'
import type {
  CheckgateClientLike, ContextExtractor, RequestFlags, RequestFlagsOptions, StartOptions, StartResult,
} from './index'

export type { RequestFlags } from './index'
export { startCheckgate, stopCheckgate } from './index'

declare global {
  // eslint-disable-next-line @typescript-eslint/no-namespace
  namespace Express {
    interface Request {
      /** The flags of this request's user, set by `checkgateMiddleware()`. */
      checkgate: RequestFlags
    }
  }
}

export interface CheckgateMiddlewareOptions<Req = IncomingMessage> extends RequestFlagsOptions {
  /** The one CheckgateClient shared by the whole app. */
  client: CheckgateClientLike
  /**
   * Returns the request's `{ key, attributes }`. Called once per request, on
   * the first evaluation. Default: `{ key: req.user.id }`, else anonymous.
   */
  context?: ContextExtractor<Req>
}

/** Sets `req.checkgate` on every request. */
export declare function checkgateMiddleware<Req = any>(
  options: CheckgateMiddlewareOptions<Req>
): (req: Req, res: unknown, next: (err?: unknown) => void) => void

export interface ListenOptions extends StartOptions {
  client: CheckgateClientLike
  /** Default 0 (any free port). */
  port?: number
  host?: string
}

export interface CheckgateServer {
  server: Server
  startup: StartResult
  /**
   * Stops accepting connections, waits for requests in flight, then
   * disconnects the client exactly once. Idempotent.
   */
  close(): Promise<void>
}

/** Starts Checkgate (waiting at most `readyTimeoutMs`), then listens. Installs no signal handlers. */
export declare function listenWithCheckgate(
  app: RequestListener | ((req: any, res: any) => void),
  options: ListenOptions
): Promise<CheckgateServer>
