#!/usr/bin/env python3
"""Upgrade, mixed-version, rollback, restore and interrupted-upgrade checks
for 0.1.31 → 0.2.0 on real databases (docs/operations/upgrading-to-0-2-0.md).

    OLD_BIN=/path/to/0.1.31/server NEW_BIN=target/release/server \\
      python3 bench/ab/upgrade_rollback_020.py [--out DIR]

Needs the PostgreSQL and Redis containers of bench/ab/README.md
($PG_CONTAINER on $PG_PORT, $REDIS_CONTAINER on $REDIS_PORT) and Node.js for
bench/public/sdk/seed.mjs. Uses only databases it creates (cg_upg_*). Every
step prints PASS/FAIL; the exit status is 1 if any step failed. Standard
library only.

Steps
 1. 0.1.31 creates the schema and data: 101 flags (seed.mjs), a second user,
    a gated environment (require_approval) with a pending PATCH change
    request, a read_write token. Snapshot fingerprint taken.
 2. Backup: pg_dump --format=custom of that database.
 3. Upgrade: 0.2.0 on the same database applies migrations 29 and 30; the
    snapshot (array and ?v=2) is byte-identical; the stream still sends
    connected/update/ready; the old SDK key and token work; the old pending
    request is an `update` and approving it applies it; a new creation in the
    gated environment is queued (`create`) and applied on approval; a
    `propose` token can be created and only queues changes.
 4. Mixed versions: 0.1.31 refuses to start against the upgraded database.
 5. Rollback (documented SQL): delete propose tokens, cancel pending
    creations, delete the two migration rows; 0.1.31 starts, serves the
    flags 0.2.0 wrote, and its own approval workflow still works (rows get
    action 'update'). Re-upgrade: 0.2.0 re-applies both migrations.
 6. Restore: the step-2 backup restored into a new database; 0.1.31 serves
    exactly the pre-upgrade snapshot (0.2.0's writes are gone, as documented).
 7. Interrupted upgrade: from the backup, 0.2.0 is killed (SIGKILL) while
    migration 30 waits on a lock. Migration 29 is applied, 30 is not, and the
    schema has no half-applied change; 0.1.31 refuses to start (as for any
    upgraded database); starting 0.2.0 again completes the upgrade.
"""

import argparse
import hashlib
import http.cookiejar
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

REPO = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True).stdout.strip()
OLD = os.environ.get("OLD_BIN")
NEW = os.environ.get("NEW_BIN", os.path.join(REPO, "target/release/server"))
PG = os.environ.get("PG_CONTAINER", "cg-gate-pg")
PG_PORT = os.environ.get("PG_PORT", "55432")
REDIS = os.environ.get("REDIS_CONTAINER", "cg-gate-redis")
REDIS_PORT = os.environ.get("REDIS_PORT", "56379")
SETUP_TOKEN = "bench-setup-token-0123456789"
MIG29, MIG30 = 20261121000029, 20261122000030
RESULTS = []


def step(name, ok, detail=""):
    RESULTS.append({"step": name, "pass": bool(ok), "detail": detail})
    print(f"{'PASS' if ok else 'FAIL'}  {name}" + (f" — {detail}" if detail else ""), flush=True)
    return ok


def psql(db, sql):
    r = subprocess.run(["docker", "exec", "-i", PG, "psql", "-U", "postgres", "-d", db, "-tAq", "-v", "ON_ERROR_STOP=1"],
                       input=sql, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"psql {db}: {r.stderr.strip()}")
    return r.stdout.strip()


def fresh_db(name):
    psql("postgres", f"DROP DATABASE IF EXISTS {name} WITH (FORCE); CREATE DATABASE {name};")


class Server:
    def __init__(self, binary, db, port, out, tag):
        self.port, self.log = port, os.path.join(out, f"{tag}.log")
        env = dict(os.environ, DATABASE_URL=f"postgres://postgres:postgres@127.0.0.1:{PG_PORT}/{db}",
                   REDIS_URL=f"redis://127.0.0.1:{REDIS_PORT}", PORT=str(port), REDIS_CHANNEL=f"cg_upg_{db}",
                   SESSION_SECRET="bench-session-secret-0123456789-0123456789-0123", SETUP_TOKEN=SETUP_TOKEN,
                   COOKIE_SECURE="false", RUST_LOG="info", PUBLIC_DIR=tempfile.mkdtemp(),
                   RATE_LIMIT_AUTH_PER_MINUTE="0", RATE_LIMIT_API_PER_MINUTE="0", RATE_LIMIT_SDK_PER_MINUTE="0",
                   RATE_LIMIT_SSE_PER_MINUTE="0", RATE_LIMIT_INGEST_PER_MINUTE="0")
        self.proc = subprocess.Popen([binary], env=env, stdout=open(self.log, "w"), stderr=subprocess.STDOUT)
        self.base = f"http://127.0.0.1:{port}"

    def ready(self, secs=60):
        end = time.time() + secs
        while time.time() < end:
            if self.proc.poll() is not None:
                return False
            try:
                with urllib.request.urlopen(self.base + "/health/ready", timeout=1) as r:
                    if r.status == 200:
                        return True
            except (urllib.error.URLError, OSError):
                pass
            time.sleep(0.2)
        return False

    def exit_code(self, secs=30):
        try:
            return self.proc.wait(secs)
        except subprocess.TimeoutExpired:
            return None

    def stop(self):
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(30)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()

    def kill9(self):
        self.proc.kill()
        self.proc.wait()

    def logtext(self):
        return open(self.log).read()


class Client:
    def __init__(self, base, bearer=None):
        self.base, self.bearer = base, bearer
        self.jar = http.cookiejar.CookieJar()
        self.opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar))

    def req(self, method, path, body=None, raw=False):
        data = json.dumps(body).encode() if body is not None else None
        r = urllib.request.Request(self.base + path, data=data, method=method)
        r.add_header("X-Checkgate-Request", "true")
        if data is not None:
            r.add_header("Content-Type", "application/json")
        if self.bearer:
            r.add_header("Authorization", f"Bearer {self.bearer}")
        try:
            with self.opener.open(r, timeout=15) as resp:
                text = resp.read()
                return resp.status, (text if raw else (json.loads(text) if text else None))
        except urllib.error.HTTPError as e:
            text = e.read()
            return e.code, (text if raw else (json.loads(text) if text.strip().startswith(b"{") or text.strip().startswith(b"[") else text.decode()))


def fingerprint(base, sdk_key, query=""):
    status, body = Client(base, sdk_key).req("GET", "/flags/snapshot" + query, raw=True)
    if status != 200:
        return None, None
    doc = json.loads(body)
    flags = doc if isinstance(doc, list) else doc["flags"]
    canon = json.dumps(sorted(flags, key=lambda f: f["key"]), sort_keys=True).encode()
    return hashlib.sha256(body).hexdigest()[:16], hashlib.sha256(canon).hexdigest()[:16]


def stream_events(base, sdk_key, secs=2.0):
    r = urllib.request.Request(base + "/stream", headers={"Authorization": f"Bearer {sdk_key}"})
    events, buf = [], b""
    try:
        with urllib.request.urlopen(r, timeout=secs) as resp:
            end = time.time() + secs
            while time.time() < end:
                chunk = resp.read1(65536) if hasattr(resp, "read1") else resp.read(65536)
                if not chunk:
                    break
                buf += chunk
                if b"event: ready" in buf:
                    break
    except (OSError, urllib.error.URLError):
        pass
    for frame in buf.split(b"\n\n"):
        for line in frame.split(b"\n"):
            if line.startswith(b"event:"):
                events.append(line[6:].strip().decode())
    return events


def migrations(db):
    return [int(v) for v in psql(db, "SELECT version FROM _sqlx_migrations ORDER BY version").split()]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(REPO, "bench/ab/out/upgrade-0.2.0"))
    a = ap.parse_args()
    if not OLD or not os.path.exists(OLD) or not os.path.exists(NEW):
        sys.exit("set OLD_BIN (0.1.31 server) and NEW_BIN (0.2.0 server)")
    os.makedirs(a.out, exist_ok=True)
    subprocess.run(["docker", "exec", REDIS, "redis-cli", "FLUSHALL"], capture_output=True)
    db = "cg_upg_main"
    fresh_db(db)

    # 1. 0.1.31 data
    old = Server(OLD, db, 3411, a.out, "1-old")
    step("1. 0.1.31 starts on an empty database", old.ready())
    session_path = os.path.join(a.out, "session.json")
    seed = subprocess.run(["node", os.path.join(REPO, "bench/public/sdk/seed.mjs"), "--url", old.base,
                           "--setup-token", SETUP_TOKEN, "--out", session_path], capture_output=True, text=True)
    step("1. seed 101 flags through the 0.1.31 API", seed.returncode == 0, seed.stderr.strip()[-200:])
    sess = json.load(open(session_path))
    sdk_key, env_id = sess["sdk_key"], sess["environment_id"]
    admin = Client(old.base)
    s, _ = admin.req("POST", "/api/auth/login", {"email": "admin@bench.test", "password": "bench-password-1"})
    step("1. admin (seed.mjs account) logs in", s == 200, f"HTTP {s}")
    _, projects = admin.req("GET", "/api/projects")
    project = projects[0]["id"]
    _, reviewer_user = admin.req("POST", "/api/users", {"name": "Rev", "email": "rev@acme.test", "role": "viewer", "password": "supersecret1"})
    admin.req("POST", f"/api/projects/{project}/members", {"user_id": reviewer_user["id"], "role": "editor"})
    _, gated = admin.req("POST", f"/api/projects/{project}/environments", {"name": "Gated", "slug": "gated"})
    gated_id = gated["id"]
    admin.req("POST", f"/api/environments/{gated_id}/flags", {"key": "gated-flag", "is_enabled": False, "rollout_percentage": None, "description": None, "rules": []})
    admin.req("POST", f"/api/projects/{project}/environments/{gated_id}/require-approval", {"require_approval": True})
    s, cr_old = admin.req("PATCH", f"/api/environments/{gated_id}/flags/gated-flag", {"is_enabled": True})
    step("1. 0.1.31 queues a PATCH in the gated environment", s == 202, f"HTTP {s}")
    s, tok = admin.req("POST", "/api/tokens", {"name": "rw", "scope": "read_write"})
    rw_token = tok.get("token") if isinstance(tok, dict) else None
    fp_old = fingerprint(old.base, sdk_key)
    fp_old_v2 = fingerprint(old.base, sdk_key, "?v=2")
    step("1. snapshot fingerprint (array, ?v=2)", fp_old[0] is not None, f"{fp_old} {fp_old_v2}")
    old.stop()
    mig_before = migrations(db)

    # 2. backup
    dump = "/tmp/cg_upg_backup.dump"
    r = subprocess.run(["docker", "exec", PG, "pg_dump", "-U", "postgres", "--format=custom", f"--file={dump}", db], capture_output=True, text=True)
    step("2. pg_dump --format=custom of the 0.1.31 database", r.returncode == 0, r.stderr.strip()[:200])

    # 3. upgrade
    new = Server(NEW, db, 3412, a.out, "3-new")
    step("3. 0.2.0 starts on the 0.1.31 database", new.ready())
    mig_after = migrations(db)
    step("3. exactly migrations 29 and 30 were applied",
         sorted(set(mig_after) - set(mig_before)) == [MIG29, MIG30], f"{len(mig_before)} → {len(mig_after)}, new {sorted(set(mig_after) - set(mig_before))}")
    step("3. snapshot byte-identical after the upgrade (array)", fingerprint(new.base, sdk_key) == fp_old, str(fingerprint(new.base, sdk_key)))
    step("3. snapshot identical after the upgrade (?v=2, flags)", fingerprint(new.base, sdk_key, "?v=2")[1] == fp_old_v2[1])
    ev = stream_events(new.base, sdk_key)
    step("3. stream protocol unchanged (connected, update…, ready)", ev[:1] == ["connected"] and "ready" in ev and "update" in ev, f"{ev[:3]}…{ev[-1:] if ev else ''}")
    s, _ = Client(new.base, rw_token).req("GET", "/api/projects")
    step("3. a token created by 0.1.31 still works", s == 200, f"HTTP {s}")
    s, crs = Client(new.base, rw_token).req("GET", f"/api/environments/{gated_id}/change-requests?status=pending")
    old_cr = next((c for c in crs if c["id"] == cr_old["id"]), None) if s == 200 else None
    step("3. the pending 0.1.31 request reads as action 'update'", old_cr is not None and old_cr.get("action") == "update", json.dumps(old_cr)[:160])
    rev = Client(new.base)
    rev.req("POST", "/api/auth/login", {"email": "rev@acme.test", "password": "supersecret1"})
    s, applied = rev.req("POST", f"/api/environments/{gated_id}/change-requests/{cr_old['id']}/approve")
    step("3. approving it (another user) applies it", s == 200 and applied.get("is_enabled") is True, f"HTTP {s}")
    admin.base = new.base
    s, cr_create = admin.req("POST", f"/api/environments/{gated_id}/flags", {"key": "made-in-020", "is_enabled": True, "rollout_percentage": 100, "description": None, "rules": []})
    step("3. enabled create in the gated environment is queued (202, action create)", s == 202 and cr_create.get("action") == "create", f"HTTP {s}")
    s, _ = rev.req("POST", f"/api/environments/{gated_id}/change-requests/{cr_create['id']}/approve")
    s2, f = admin.req("GET", f"/api/environments/{gated_id}/flags/made-in-020")
    step("3. approving the creation creates the flag", s == 200 and s2 == 200 and f.get("is_enabled") is True, f"approve {s}, get {s2}")
    s, pending_create = admin.req("POST", f"/api/environments/{gated_id}/flags", {"key": "left-pending", "is_enabled": True, "rollout_percentage": None, "description": None, "rules": []})
    step("3. a second creation is left pending (for the rollback step)", s == 202)
    s, ptok = admin.req("POST", "/api/tokens", {"name": "agent", "scope": "propose"})
    agent = Client(new.base, ptok.get("token") if isinstance(ptok, dict) else None)
    s1, _ = agent.req("PATCH", f"/api/environments/{env_id}/flags/hot", {"default_value": "agent"})
    s2, _ = agent.req("POST", f"/api/environments/{env_id}/flags", {"key": "agent-new", "is_enabled": False, "rollout_percentage": None, "description": None, "rules": []})
    step("3. a propose token queues PATCHes (202) and cannot create (403)", s1 == 202 and s2 == 403, f"PATCH {s1}, POST {s2}")

    # 4. mixed versions
    mixed = Server(OLD, db, 3413, a.out, "4-mixed-old")
    code = mixed.exit_code(30)
    text = mixed.logtext()
    step("4. 0.1.31 refuses to start next to 0.2.0 (rolling upgrade unsupported)",
         code not in (None, 0) and "previously applied but is missing" in text, f"exit {code}; {'migration message present' if 'previously applied but is missing' in text else text[-200:]}")
    if code is None:
        mixed.stop()
    new.stop()

    # 5. rollback by SQL
    psql(db, f"""
        DELETE FROM personal_access_tokens WHERE scope = 'propose';
        UPDATE change_requests SET status = 'cancelled' WHERE action = 'create' AND status = 'pending';
        DELETE FROM _sqlx_migrations WHERE version IN ({MIG29}, {MIG30});""")
    back = Server(OLD, db, 3414, a.out, "5-rollback-old")
    step("5. after the documented SQL, 0.1.31 starts on the database", back.ready())
    admin.base = back.base
    s, f = admin.req("GET", f"/api/environments/{gated_id}/flags/made-in-020")
    step("5. 0.1.31 serves the flag 0.2.0 created", s == 200 and f.get("is_enabled") is True, f"HTTP {s}")
    s, crs = admin.req("GET", f"/api/environments/{gated_id}/change-requests")
    lp = [c for c in crs if c["flag_key"] == "left-pending"] if s == 200 else []
    step("5. the pending creation is cancelled, not approvable", lp and all(c["status"] == "cancelled" for c in lp), json.dumps(lp)[:160])
    s, _ = Client(back.base, rw_token).req("GET", "/api/projects")
    step("5. read_write tokens still work on 0.1.31", s == 200)
    s, cr_back = admin.req("PATCH", f"/api/environments/{gated_id}/flags/made-in-020", {"rollout_percentage": 50})
    rev.base = back.base
    rev.req("POST", "/api/auth/login", {"email": "rev@acme.test", "password": "supersecret1"})
    s2, _ = rev.req("POST", f"/api/environments/{gated_id}/change-requests/{cr_back.get('id')}/approve") if s == 202 else (None, None)
    action = psql(db, f"SELECT action FROM change_requests WHERE id = {int(cr_back.get('id', 0))}") if s == 202 else ""
    step("5. 0.1.31's own approval workflow works (row gets action 'update')", s == 202 and s2 == 200 and action == "update", f"PATCH {s}, approve {s2}, action {action!r}")
    step("5. no propose tokens remain", psql(db, "SELECT count(*) FROM personal_access_tokens WHERE scope = 'propose'") == "0")
    back.stop()
    again = Server(NEW, db, 3415, a.out, "5-reupgrade-new")
    step("5. re-upgrade: 0.2.0 starts and re-applies both migrations", again.ready() and {MIG29, MIG30} <= set(migrations(db)))
    again.stop()

    # 6. restore from the backup
    rdb = "cg_upg_restore"
    fresh_db(rdb)
    r = subprocess.run(["docker", "exec", PG, "pg_restore", "-U", "postgres", "-d", rdb, "--no-owner", dump], capture_output=True, text=True)
    step("6. pg_restore of the pre-upgrade backup", r.returncode == 0, r.stderr.strip()[:200])
    restored = Server(OLD, rdb, 3416, a.out, "6-restore-old")
    step("6. 0.1.31 starts on the restored database", restored.ready())
    step("6. it serves exactly the pre-upgrade snapshot", fingerprint(restored.base, sdk_key) == fp_old)
    s, _ = Client(restored.base, sdk_key).req("GET", "/flags/snapshot")
    admin.base = restored.base
    s, _ = admin.req("GET", f"/api/environments/{gated_id}/flags/made-in-020")
    step("6. 0.2.0's writes are not in the restored database (documented loss)", s == 404, f"HTTP {s}")
    restored.stop()

    # 7. interrupted upgrade
    idb = "cg_upg_interrupt"
    fresh_db(idb)
    subprocess.run(["docker", "exec", PG, "pg_restore", "-U", "postgres", "-d", idb, "--no-owner", dump], capture_output=True)
    locker = subprocess.Popen(["docker", "exec", "-i", PG, "psql", "-U", "postgres", "-d", idb, "-v", "ON_ERROR_STOP=1"],
                              stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, text=True)
    locker.stdin.write("BEGIN; LOCK TABLE change_requests IN ACCESS EXCLUSIVE MODE; SELECT pg_sleep(60);\n")
    locker.stdin.flush()
    time.sleep(1.5)
    victim = Server(NEW, idb, 3417, a.out, "7-interrupted-new")
    blocked = False
    for _ in range(100):
        waiting = psql("postgres", f"SELECT count(*) FROM pg_stat_activity WHERE datname = '{idb}' AND wait_event_type = 'Lock'")
        if waiting not in ("", "0"):
            blocked = True
            break
        time.sleep(0.1)
    step("7. 0.2.0 is blocked inside the upgrade (migration 30 waits on the lock)", blocked)
    victim.kill9()
    psql("postgres", f"SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{idb}' AND pid <> pg_backend_pid()")
    locker.kill()
    time.sleep(0.5)
    applied = set(migrations(idb))
    col = psql(idb, "SELECT count(*) FROM information_schema.columns WHERE table_name = 'change_requests' AND column_name = 'action'")
    step("7. after SIGKILL: 29 applied, 30 not, and no half-applied column",
         MIG29 in applied and MIG30 not in applied and col == "0", f"29 {'yes' if MIG29 in applied else 'no'}, 30 {'yes' if MIG30 in applied else 'no'}, action column {col}")
    refused = Server(OLD, idb, 3418, a.out, "7-old-after-interrupt")
    code = refused.exit_code(30)
    step("7. 0.1.31 refuses the partly upgraded database (finish or roll back)", code not in (None, 0), f"exit {code}")
    if code is None:
        refused.stop()
    resumed = Server(NEW, idb, 3419, a.out, "7-resumed-new")
    ok = resumed.ready()
    step("7. starting 0.2.0 again completes the upgrade", ok and {MIG29, MIG30} <= set(migrations(idb)))
    step("7. data intact after the resumed upgrade", fingerprint(resumed.base, sdk_key) == fp_old)
    resumed.stop()

    for name in ("cg_upg_main", "cg_upg_restore", "cg_upg_interrupt"):
        psql("postgres", f"DROP DATABASE IF EXISTS {name} WITH (FORCE)")
    subprocess.run(["docker", "exec", PG, "rm", "-f", dump], capture_output=True)
    json.dump({"old_bin": OLD, "new_bin": NEW, "results": RESULTS}, open(os.path.join(a.out, "results.json"), "w"), indent=2)
    failed = [r for r in RESULTS if not r["pass"]]
    print(f"\n{len(RESULTS) - len(failed)}/{len(RESULTS)} passed; results in {a.out}/results.json")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
