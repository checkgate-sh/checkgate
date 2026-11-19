// A deterministic in-memory stand-in for the Rust engine, for suites that run
// without a native or WASM build (the CI unit jobs). It evaluates enough of a
// flag (enabled, rollout, one attribute rule) for the flapping invariant to
// mean something; the real engine's semantics are tested in core/ and by the
// live suites.

function bucket(flagKey, userKey) {
    let h = 2166136261;
    for (const c of `${flagKey}:${userKey}`) h = Math.imul(h ^ c.charCodeAt(0), 16777619) >>> 0;
    return h % 100;
}

export function makeFakeCore() {
    const flags = new Map();
    const evaluate = (key, user, attrs = {}) => {
        const f = flags.get(key);
        if (!f || !f.is_enabled) return false;
        for (const rule of f.rules || []) {
            if (rule.attribute && rule.values && rule.values.includes(attrs[rule.attribute])) return true;
        }
        return f.rollout_percentage == null || bucket(key, user) < f.rollout_percentage;
    };
    return {
        flags,
        upsert(flag) { flags.set(flag.key, flag); },
        delete(key) { flags.delete(key); },
        isEnabled: evaluate,
    };
}
