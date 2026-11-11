// Benchmark datasets. Deterministic: no randomness, no clock.
//
// - serverFlags(): what the server benchmarks seed through the REST API —
//   100 flags, a quarter each of boolean, 25 % rollout, one targeting rule and
//   a two-way multivariate (the same mix as server/examples/loadgen.rs), plus
//   `hot`, the string flag the propagation benchmark rewrites.
// - evaluationFlags(): the 1 000-flag store of the SDK evaluation benchmark,
//   with the same special flags, rules and user as bench/benches/evaluation.rs.

export const HOT_FLAG = 'hot';
export const SERVER_FLAG_COUNT = 101;

export function serverFlags() {
  const flags = [];
  for (let i = 0; i < 100; i++) {
    const key = `flag-${i}`;
    switch (i % 4) {
      case 0:
        flags.push({ key, is_enabled: true, flag_type: 'boolean', rules: [] });
        break;
      case 1:
        flags.push({ key, is_enabled: true, rollout_percentage: 25, flag_type: 'boolean', rules: [] });
        break;
      case 2:
        flags.push({
          key, is_enabled: true, flag_type: 'boolean',
          rules: [{ attribute: 'email', operator: 'ends_with', values: ['@acme.test'] }],
        });
        break;
      default:
        flags.push({
          key, is_enabled: true, flag_type: 'string', default_value: 'a',
          variants: [{ weight: 50, value: 'a' }, { weight: 50, value: 'b' }], rules: [],
        });
    }
  }
  flags.push({ key: HOT_FLAG, is_enabled: true, flag_type: 'string', default_value: 'u0', rules: [] });
  return flags;
}

/** The representative user of bench/benches/evaluation.rs. */
export const USER_KEY = 'user-8f14e45f-ceea-467f-a0e6-5e7d1c2b9a31';
export const USER_ATTRIBUTES = Object.freeze({
  email: 'dana@example.test',
  plan: 'enterprise',
  country: 'PH',
  app_version: '4.12.0',
  platform: 'android',
});

/** `rules` rules that do not match the user, then one that does. */
function targeted(key, rules) {
  const r = [];
  for (let i = 0; i < rules - 1; i++) {
    r.push({ attribute: 'email', operator: 'ends_with', values: [`@other${i}.test`] });
  }
  r.push({ attribute: 'plan', operator: 'equals', values: ['enterprise'], variant: true });
  return { key, is_enabled: true, flag_type: 'boolean', rules: r };
}

export function evaluationFlags() {
  const flags = [];
  for (let i = 0; i < 1000; i++) {
    flags.push({ key: `flag-${i}`, is_enabled: true, flag_type: 'boolean', rules: [] });
  }
  flags.push(targeted('t1', 1));
  flags.push(targeted('t10', 10));
  flags.push({ key: 'rollout', is_enabled: true, rollout_percentage: 50, flag_type: 'boolean', rules: [] });
  flags.push({
    key: 'mv', is_enabled: true, flag_type: 'string', rules: [],
    default_value: 'control', disabled_value: 'off',
    variants: [{ weight: 60, value: 'a' }, { weight: 30, value: 'b' }, { weight: 10, value: 'c' }],
  });
  // A chain of 5: chain-0 requires chain-1 … requires chain-4.
  for (let i = 0; i < 5; i++) {
    flags.push({
      key: `chain-${i}`, is_enabled: true, flag_type: 'boolean', rules: [],
      prerequisites: i + 1 < 5 ? [{ flag_key: `chain-${i + 1}` }] : [],
    });
  }
  return flags;
}
