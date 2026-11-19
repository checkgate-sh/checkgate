// A scriptable stand-in for the Checkgate server's SDK endpoints (`/stream`
// and `/flags/snapshot`), speaking the same wire protocol, with network faults
// that can be switched on between any two bytes. Every SDK's fault-injection
// suite runs against it (see README.md); the real server's own fault tests are
// in server/tests/sse_faults.rs.
//
// It holds the authoritative flag set and the full revision history, so a
// test can tell exactly which revision of a flag a client is serving.

import http from 'node:http';

const ENV_ID = '00000000-0000-4000-8000-00000000c0f1';

/** SSE frame for one event. */
function frame(event, data) {
    return `event: ${event}\ndata: ${typeof data === 'string' ? data : JSON.stringify(data)}\n\n`;
}

export class FaultServer {
    /**
     * @param {object} [opts]
     * @param {number} [opts.heartbeatMs=100] - heartbeat interval announced and sent
     * @param {string} [opts.sdkKey='sk_conformance']
     * @param {boolean} [opts.announceHeartbeat=true] - false behaves like a pre-0.1.31
     *        server: no `heartbeat_ms` and comment keep-alives instead of events
     * @param {boolean} [opts.legacySnapshot=false] - true behaves like a pre-0.1.31
     *        server: `/flags/snapshot` ignores `?v=2` and sends no revisions
     */
    constructor({ heartbeatMs = 100, sdkKey = 'sk_conformance', announceHeartbeat = true, legacySnapshot = false } = {}) {
        this.heartbeatMs = heartbeatMs;
        this.sdkKey = sdkKey;
        this.announceHeartbeat = announceHeartbeat;
        this.legacySnapshot = legacySnapshot;
        this.revision = 0;
        /** key -> { flag, revision } */
        this.flags = new Map();
        /** key -> revision of the delete */
        this.tombstones = new Map();
        /** key -> Map(revision -> flag | null (deleted)) */
        this.history = new Map();
        this.streams = new Set();
        // New connections: 'pass' | 'refuse' (socket destroyed) | 'unavailable' (503)
        this.mode = 'pass';
        this.acceptedKey = sdkKey;
        this.fragment = false;
        this.writeDelayMs = 0;
        this.snapshotDelayMs = 0;
        this.pending = { truncateNext: false, duplicateNext: false, staleAfterReady: false, cutReplay: false, holdMs: 0 };
        /** Set once a stream held by holdNextStream() has been opened (its replay read). */
        this.heldStreamOpened = false;
        this.counters = {
            streamRequests: 0, streamsOpened: 0, maxConcurrentStreams: 0,
            snapshotRequests: 0, unauthorized: 0, heartbeats: 0, bytesSent: 0,
        };
        /** Timestamps (ms) when a stream was accepted, for detection timing. */
        this.streamOpenedAt = [];
    }

    async start() {
        this.server = http.createServer((req, res) => this._handle(req, res));
        this.server.keepAliveTimeout = 1000;
        await new Promise((resolve) => this.server.listen(0, '127.0.0.1', resolve));
        this.url = `http://127.0.0.1:${this.server.address().port}`;
        return this.url;
    }

    async stop() {
        for (const s of this.streams) s.kill();
        this.streams.clear();
        if (!this.server) return;
        this.server.closeAllConnections?.();
        await new Promise((resolve) => this.server.close(() => resolve()));
    }

    // -- authoritative state ------------------------------------------------

    _record(key, revision, flag) {
        if (!this.history.has(key)) this.history.set(key, new Map());
        this.history.get(key).set(revision, flag);
    }

    /** Creates or replaces a flag; its description carries the revision. */
    upsert(flag) {
        const revision = ++this.revision;
        const stored = { ...flag, description: `rev:${revision}` };
        this.flags.set(flag.key, { flag: stored, revision });
        this.tombstones.delete(flag.key);
        this._record(flag.key, revision, stored);
        this._broadcast({ type: 'UPSERT', env_id: ENV_ID, flag: stored, revision });
        return revision;
    }

    delete(key) {
        if (!this.flags.has(key)) return null;
        const revision = ++this.revision;
        this.flags.delete(key);
        this.tombstones.set(key, revision);
        this._record(key, revision, null);
        this._broadcast({ type: 'DELETE', env_id: ENV_ID, key, revision });
        return revision;
    }

    /** The flag set a client must converge to: key -> flag. */
    authoritative() {
        return new Map(Array.from(this.flags, ([k, v]) => [k, v.flag]));
    }

    // -- faults -------------------------------------------------------------

    /** Closes every open stream at once (a dropped link or a restart). */
    severAll() { for (const s of this.streams) s.kill(); }
    /** Every open stream goes silent forever, sockets left open (half-open / lost heartbeats). */
    stallAll() { for (const s of this.streams) s.stalled = true; }
    /** The next event on each open stream is cut mid-frame, then the socket closes. */
    truncateNext() { this.pending.truncateNext = true; }
    /** The next event is delivered twice. */
    duplicateNext() { this.pending.duplicateNext = true; }
    /** The next connection's replay is cut mid-frame before "ready". */
    cutNextReplay() { this.pending.cutReplay = true; }
    /** After the next "ready", replay an older revision of a flag (reordered delivery). */
    staleAfterNextReady() { this.pending.staleAfterReady = true; }
    /**
     * The next stream's replay is read when it opens, as the server does, but
     * none of its bytes leave for `ms` (a slow or buffering link): polls can
     * overtake it (R20).
     */
    holdNextStream(ms) { this.pending.holdMs = ms; this.heldStreamOpened = false; }
    /** Credentials: the key that is accepted (null rejects every key with 401). */
    setAcceptedKey(key) { this.acceptedKey = key; }

    // -- HTTP ---------------------------------------------------------------

    _authorized(req, url) {
        const header = req.headers.authorization || '';
        const key = header.startsWith('Bearer ') ? header.slice(7) : url.searchParams.get('sdk_key');
        return this.acceptedKey !== null && key === this.acceptedKey;
    }

    _handle(req, res) {
        const url = new URL(req.url, 'http://x');
        if (url.pathname === '/stream') return this._stream(req, res, url);
        if (url.pathname === '/flags/snapshot') return this._snapshot(req, res, url);
        if (url.pathname.startsWith('/api/environments/')) {
            res.writeHead(202).end();
            return;
        }
        res.writeHead(404).end();
    }

    _gate(req, res, url) {
        if (this.mode === 'refuse') {
            req.socket.destroy();
            return false;
        }
        if (this.mode === 'unavailable') {
            res.writeHead(503, { 'Retry-After': '1' }).end('busy');
            return false;
        }
        if (!this._authorized(req, url)) {
            this.counters.unauthorized++;
            res.writeHead(401).end();
            return false;
        }
        return true;
    }

    _snapshot(req, res, url) {
        this.counters.snapshotRequests++;
        if (!this._gate(req, res, url)) return;
        // The body is taken now and sent later: a slow poll carries old state.
        // `?v=2` gets revisions and tombstones, as from a 0.1.31 server; a
        // server with `legacySnapshot` (before 0.1.31) ignores it.
        const flags = Array.from(this.flags.values(), (v) => v.flag);
        const body = JSON.stringify(url.searchParams.get('v') === '2' && !this.legacySnapshot
            ? {
                v: 2,
                flags,
                revisions: Object.fromEntries(Array.from(this.flags, ([k, v]) => [k, v.revision])),
                tombstones: Object.fromEntries(this.tombstones),
            }
            : flags);
        const send = () => {
            if (res.destroyed) return;
            res.writeHead(200, { 'Content-Type': 'application/json', 'X-Checkgate-Environment-Id': ENV_ID });
            res.end(body);
        };
        if (this.snapshotDelayMs > 0) setTimeout(send, this.snapshotDelayMs);
        else send();
    }

    _stream(req, res, url) {
        this.counters.streamRequests++;
        if (!this._gate(req, res, url)) return;
        res.writeHead(200, {
            'Content-Type': 'text/event-stream',
            'Cache-Control': 'no-cache',
            Connection: 'keep-alive',
        });
        res.flushHeaders();
        const stream = new Stream(this, req, res);
        if (this.pending.holdMs > 0) {
            const ms = this.pending.holdMs;
            this.pending.holdMs = 0;
            stream.chain = new Promise((r) => setTimeout(r, ms));
            this.heldStreamOpened = true;
        }
        this.streams.add(stream);
        this.counters.streamsOpened++;
        this.streamOpenedAt.push(Date.now());
        this.counters.maxConcurrentStreams = Math.max(this.counters.maxConcurrentStreams, this.streams.size);
        req.on('close', () => stream.kill());

        stream.write(frame('connected', this.announceHeartbeat
            ? { environment_id: ENV_ID, heartbeat_ms: this.heartbeatMs }
            : { environment_id: ENV_ID }));
        const replay = [];
        for (const [, v] of [...this.flags].sort(([a], [b]) => (a < b ? -1 : 1))) {
            replay.push(frame('update', { type: 'UPSERT', env_id: ENV_ID, flag: v.flag, revision: v.revision }));
        }
        for (const [key, revision] of this.tombstones) {
            replay.push(frame('update', { type: 'DELETE', env_id: ENV_ID, key, revision }));
        }
        if (this.pending.cutReplay && replay.length > 0) {
            this.pending.cutReplay = false;
            const cut = Math.floor(replay.length / 2);
            replay.slice(0, cut).forEach((f) => stream.write(f));
            stream.writeCut(replay[cut]);
            return;
        }
        replay.forEach((f) => stream.write(f));
        stream.write(frame('ready', String(this.flags.size)));
        if (this.pending.staleAfterReady) {
            this.pending.staleAfterReady = false;
            const older = this._olderRevision();
            if (older) stream.write(frame('update', older));
        }
    }

    /** An event for some flag at a revision older than the current one, if any. */
    _olderRevision() {
        for (const [key, versions] of this.history) {
            const revs = [...versions.keys()].sort((a, b) => a - b);
            if (revs.length < 2) continue;
            const old = revs[revs.length - 2];
            const flag = versions.get(old);
            return flag
                ? { type: 'UPSERT', env_id: ENV_ID, flag, revision: old }
                : { type: 'DELETE', env_id: ENV_ID, key, revision: old };
        }
        return null;
    }

    _broadcast(event) {
        const f = frame('update', event);
        const truncate = this.pending.truncateNext;
        const duplicate = this.pending.duplicateNext;
        this.pending.truncateNext = false;
        this.pending.duplicateNext = false;
        for (const s of this.streams) {
            if (truncate) {
                s.writeCut(f);
                continue;
            }
            s.write(f);
            if (duplicate) s.write(f);
        }
    }
}

/** One open `/stream` response, with ordered (optionally fragmented or delayed) writes. */
class Stream {
    constructor(server, req, res) {
        this.server = server;
        this.req = req;
        this.res = res;
        this.stalled = false;
        this.dead = false;
        this.chain = Promise.resolve();
        this.heartbeat = setInterval(() => {
            if (this.stalled || this.dead) return;
            server.counters.heartbeats++;
            // Before 0.1.31 the keep-alive was a comment, invisible to EventSource.
            this.write(server.announceHeartbeat ? frame('heartbeat', '{}') : ':keep-alive-text\n\n');
        }, server.heartbeatMs);
    }

    _raw(text) {
        if (this.dead || this.stalled) return Promise.resolve();
        const server = this.server;
        const bytes = Buffer.from(text);
        server.counters.bytesSent += bytes.length;
        // Fault settings are read live, so healing also speeds up a write
        // that is already under way.
        const pause = () => new Promise((r) => (server.writeDelayMs > 0 ? setTimeout(r, server.writeDelayMs) : setImmediate(r)));
        if (!server.fragment && server.writeDelayMs === 0) {
            this.res.write(bytes);
            return Promise.resolve();
        }
        return (async () => {
            if (!server.fragment) {
                await pause();
                if (!this.dead && !this.stalled) this.res.write(bytes);
                return;
            }
            for (let i = 0; i < bytes.length; i++) {
                if (this.dead || this.stalled) return;
                if (!server.fragment) {
                    this.res.write(bytes.subarray(i));
                    return;
                }
                this.res.write(bytes.subarray(i, i + 1));
                await pause();
            }
        })();
    }

    write(text) {
        this.chain = this.chain.then(() => this._raw(text));
    }

    /** Writes part of a frame, then drops the connection: an interrupted event. */
    writeCut(text) {
        const cut = Math.max(1, Math.floor(text.length / 2));
        this.chain = this.chain.then(() => this._raw(text.slice(0, cut))).then(() => this.kill());
    }

    kill() {
        if (this.dead) return;
        this.dead = true;
        clearInterval(this.heartbeat);
        this.server.streams.delete(this);
        this.res.destroy();
        this.req.socket.destroy();
    }
}

export { ENV_ID };
