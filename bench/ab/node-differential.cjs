// Differential check between two builds of the Node SDK's native addon:
// the same 300 generated flags and 400 generated contexts through both, every
// result compared. Usage: node bench/ab/node-differential.cjs BASE.node CAND.node
const path = require('node:path');
const base = require(path.resolve(process.argv[2]));
const cand = require(path.resolve(process.argv[3]));
const A = new base.CheckgateCore(), B = new cand.CheckgateCore();
let seed = 12345; const rnd = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
const pick = (a) => a[Math.floor(rnd() * a.length)];
const ATTRS = ['plan', 'country', 'email', 'age', 'beta', 'constructor', 'toString', ''];
const OPS = ['equals','not_equals','contains','starts_with','ends_with','greater_than','less_than_or_equal'];
const VALS = ['pro','free','PH','US','@acme.test','a','30','18','x',''];
const flags = [];
for (let i = 0; i < 300; i++) {
  const rules = Array.from({ length: Math.floor(rnd() * 4) }, () => ({
    attribute: pick(ATTRS), operator: pick(OPS), values: [pick(VALS), pick(VALS)],
    ...(rnd() < 0.3 ? { variant: pick(['red','blue']) } : {}) }));
  const prerequisites = i > 5 && rnd() < 0.4
    ? Array.from({ length: 1 + Math.floor(rnd() * 2) }, () => ({ flag_key: `f${Math.floor(rnd() * i)}` })) : [];
  const f = { key: `f${i}`, is_enabled: rnd() < 0.85, rollout_percentage: pick([null, 0, 10, 50, 100]),
    description: null, rules, prerequisites, flag_type: rnd() < 0.5 ? 'boolean' : 'string',
    ...(rnd() < 0.4 ? { variants: [{ weight: 60, value: 'a' }, { weight: 40, value: 'b' }] } : {}) };
  flags.push(f);
  A.upsertFlagV2(JSON.stringify(f)); B.upsertFlagV2(JSON.stringify(f));
}
let n = 0, mismatch = 0, bothThrew = 0, onlyBaseThrew = 0, onlyCandThrew = 0;
const call = (core, m, k, u, a) => { try { return { v: core[m](k, u, a) }; } catch (e) { return { e: e.constructor.name } } };
for (let c = 0; c < 400; c++) {
  const attrs = {};
  for (const name of ATTRS) if (rnd() < 0.6) attrs[name] = pick(VALS);
  if (rnd() < 0.15) attrs.extraNumber = 42;          // unreferenced non-string
  if (rnd() < 0.05) attrs[pick(ATTRS)] = 7;           // possibly referenced non-string
  if (rnd() < 0.1) attrs[pick(ATTRS)] = undefined;
  const user = `user-${c}`;
  for (const f of flags) for (const m of ['isEnabled', 'getValue', 'getVariant']) {
    n++;
    const a = call(A, m, f.key, user, attrs), b = call(B, m, f.key, user, attrs);
    if (a.e && b.e) bothThrew++;
    else if (a.e && !b.e) onlyBaseThrew++;
    else if (!a.e && b.e) { onlyCandThrew++; if (onlyCandThrew < 4) console.log('cand threw', m, JSON.stringify(f), JSON.stringify(attrs)); }
    else if (a.v !== b.v) { mismatch++; if (mismatch < 4) console.log('MISMATCH', m, f.key, JSON.stringify(attrs), a.v, b.v); }
  }
}
console.log(JSON.stringify({ calls: n, mismatches: mismatch, both_threw: bothThrew, only_base_threw: onlyBaseThrew, only_candidate_threw: onlyCandThrew }));
