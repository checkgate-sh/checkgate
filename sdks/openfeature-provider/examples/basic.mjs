// Run against a local Checkgate server:
//   CHECKGATE_URL=http://localhost:3000 CHECKGATE_SDK_KEY=<development key> node examples/basic.mjs
import { OpenFeature } from '@openfeature/server-sdk';
import { CheckgateProvider } from '@checkgate/openfeature-provider';

const provider = new CheckgateProvider({
  serverUrl: process.env.CHECKGATE_URL ?? 'http://localhost:3000',
  sdkKey: process.env.CHECKGATE_SDK_KEY,
  initTimeoutMs: 5_000,
});

await OpenFeature.setProviderAndWait(provider);
const client = OpenFeature.getClient();

const details = await client.getBooleanDetails('new-checkout', false, { targetingKey: 'user-123', plan: 'pro' });
console.log(details); // { value, reason, variant, flagMetadata: { checkgateReason, ... } }

await OpenFeature.close();
