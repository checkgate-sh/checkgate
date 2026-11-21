// Evaluates `new-checkout` for two users every second, locally, and prints
// when the answer changes. Toggle the flag in the dashboard and watch.
import { CheckgateClient } from '@checkgate/node';

const client = new CheckgateClient({
  serverUrl: `http://localhost:${process.env.CHECKGATE_PORT || 3000}`,
  sdkKey: process.env.CHECKGATE_SDK_KEY,
  reportImpressions: false,
});
await client.connect();
console.log('Connected. Toggle new-checkout in the dashboard (Ctrl-C to stop).');

const users = { 'user-pro': { plan: 'pro' }, 'user-free': { plan: 'free' } };
let last = '';
const show = () => {
  const now = Object.entries(users).map(([id, attrs]) => `${id}=${client.isEnabled('new-checkout', id, attrs)}`).join('  ');
  if (now !== last) console.log(`${new Date().toISOString()}  new-checkout: ${now}`);
  last = now;
};
show();
client.onChange(show);
setInterval(show, 1000);
process.on('SIGINT', () => { client.disconnect(); process.exit(0); });
