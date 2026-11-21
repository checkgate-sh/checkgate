// Type-checked by `npm run typecheck` (never executed): the README example,
// compiled against the real OpenFeature and Checkgate declarations.
import { OpenFeature } from '@openfeature/server-sdk';
import { CheckgateProvider } from '@checkgate/openfeature-provider';

export async function main(): Promise<boolean> {
  const provider = new CheckgateProvider({
    serverUrl: process.env.CHECKGATE_URL!,
    sdkKey: process.env.CHECKGATE_SDK_KEY!,
    initTimeoutMs: 5_000,
  });

  await OpenFeature.setProviderAndWait(provider);
  const client = OpenFeature.getClient();

  const enabled: boolean = await client.getBooleanValue('new-checkout', false, {
    targetingKey: 'user-123',
    plan: 'pro',
  });
  const details = await client.getStringDetails('checkout-theme', 'classic', { targetingKey: 'user-123' });
  const reason: string | undefined = details.reason;
  void reason;
  await OpenFeature.close();
  return enabled;
}
