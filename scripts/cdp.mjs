// Drives the running setup's page (its WebView2) through the DevTools Protocol, for checks without
// clicking. Adapted from Jay-S's cdp-eval.mjs and cdp-shot.mjs. Start the setup with
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9228 first.
//
//   node scripts/cdp.mjs eval "<js expression>"     prints the (awaited) value as JSON
//   node scripts/cdp.mjs shot <out.png> [dark|light] saves a PNG of the page only, never the screen
//   (CDP_MOTION=no-preference|reduce emulates prefers-reduced-motion for the shot; CDP_PORT, default 9228)
const [, , action, arg, scheme] = process.argv;
const port = process.env.CDP_PORT ?? '9228';
if (!['eval', 'shot'].includes(action) || !arg) {
  console.error('usage: node scripts/cdp.mjs eval "<js>" | shot <out.png> [dark|light]');
  process.exit(2);
}

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = targets.find((t) => t.type === 'page' && /^https?:\/\/tauri\.localhost\//.test(t.url));
if (!page) throw new Error(`no setup page among: ${targets.map((t) => t.url).join(', ')}`);

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.onopen = resolve;
  ws.onerror = reject;
});
let next = 0;
const call = (method, params = {}) =>
  new Promise((resolve) => {
    const id = ++next;
    ws.addEventListener('message', function on(event) {
      const msg = JSON.parse(event.data);
      if (msg.id !== id) return;
      ws.removeEventListener('message', on);
      resolve(msg);
    });
    ws.send(JSON.stringify({ id, method, params }));
  });

if (action === 'eval') {
  const reply = await call('Runtime.evaluate', { expression: arg, awaitPromise: true, returnByValue: true, userGesture: true });
  const { result, exceptionDetails } = reply.result ?? {};
  ws.close();
  if (exceptionDetails) {
    console.error('EXCEPTION', exceptionDetails.exception?.description ?? exceptionDetails.text);
    process.exit(1);
  }
  console.log(JSON.stringify(result?.value, null, 2));
} else {
  const { writeFileSync } = await import('node:fs');
  // The emulation lasts while this connection is open, so set it and shoot in the same session.
  const features = [];
  if (scheme) features.push({ name: 'prefers-color-scheme', value: scheme });
  if (process.env.CDP_MOTION) features.push({ name: 'prefers-reduced-motion', value: process.env.CDP_MOTION });
  if (features.length) await call('Emulation.setEmulatedMedia', { features });
  await new Promise((r) => setTimeout(r, 300));
  const reply = await call('Page.captureScreenshot', { format: 'png' });
  ws.close();
  if (!reply.result) throw new Error(JSON.stringify(reply.error));
  writeFileSync(arg, Buffer.from(reply.result.data, 'base64'));
  console.log(arg);
}
