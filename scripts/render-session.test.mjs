import test from 'node:test';
import assert from 'node:assert/strict';
import {
  cleanupMissingDevice, createIntentGuard, createPreviewRenderer,
  createRenderSession, decodePreviewFrame,
} from '../src/renderSession.ts';

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
async function flush() { for (let i = 0; i < 40; i++) await Promise.resolve(); }
function manualClock() {
  let sequence = 0;
  const timers = new Map();
  return {
    schedule(fn, ms) { const id = ++sequence; timers.set(id, { fn, ms }); return id; },
    cancel(id) { timers.delete(id); },
    fire(ms) {
      for (const [id, timer] of [...timers]) {
        if (timer.ms === ms) { timers.delete(id); timer.fn(); }
      }
    },
  };
}
const idle = () => ({ sessionId: null, requestId: null, phase: 'idle', active: false, open: false, ready: false, serial: null });
const running = (id, requestId, phase = 'running') => ({
  sessionId: id, requestId, phase, active: true, open: true, ready: phase === 'running', serial: 'device-A',
});
const args = { html: 'assets/theme/v1p/index.html', serial: 'device-A', width: 320, height: 170 };
function harness() {
  const calls = [];
  const handlers = new Map();
  const clock = manualClock();
  let state = idle();
  let sequence = 0;
  const views = [];
  const session = createRenderSession({
    timeoutMs: 30, queryTimeoutMs: 3, clock,
    requestId: () => `request-${++sequence}`,
    changed: (view) => views.push(view),
    async invoke(command, payload) {
      calls.push({ command, payload });
      if (handlers.has(command)) return handlers.get(command)(payload);
      if (command === 'render_state') return state;
      if (command === 'render_cancel') return state;
      if (command === 'render_release') return '仍在收尾';
      if (command === 'render_start') { state = running(sequence, payload.requestId); return 'started'; }
      throw new Error(`Unexpected command: ${command}`);
    },
  });
  return { session, calls, handlers, clock, views,
    set state(value) { state = value; }, get state() { return state; } };
}

test('startup timeout targets its request; stopping is not idle and cannot admit another start', async () => {
  const h = harness();
  await h.session.refresh();
  const late = deferred();
  h.handlers.set('render_start', () => late.promise);
  const start = h.session.start(args);
  await flush();
  assert.equal(h.session.view().canStart, false);
  // A pre-registration idle poll cannot unlock admission.
  await h.session.refresh();
  assert.equal(h.session.view().canStart, false);
  h.state = running(1, 'request-1', 'stopping');
  h.clock.fire(30);
  await flush();
  assert.equal(await start, false);
  const cancel = h.calls.find((call) => call.command === 'render_cancel');
  assert.deepEqual(cancel.payload, { requestId: 'request-1' });
  assert.ok(h.calls.some((call) => call.command === 'render_release'));
  assert.equal(h.session.view().active, true);
  assert.equal(h.session.view().canStart, false);
  assert.equal(await h.session.start(args), false);
  assert.equal(h.calls.filter((call) => call.command === 'render_start').length, 1);
  h.state = idle();
  assert.equal(await h.session.refresh(), true);
  h.handlers.delete('render_start');
  assert.equal(await h.session.start(args), true);
  const before = h.session.view();
  late.resolve('old request finally returned');
  await flush();
  assert.deepEqual(h.session.view(), before);
  assert.equal(h.session.view().previewSession, 2);
  h.session.dispose();
});

test('explicit stop supersedes an unresolved start; late success does not replace the new intent', async () => {
  const h = harness();
  await h.session.refresh();
  const old = deferred();
  h.handlers.set('render_start', () => old.promise);
  const first = h.session.start(args);
  await flush();
  assert.equal(await h.session.stop(), true);
  h.handlers.delete('render_start');
  assert.equal(await h.session.start(args), true);
  const view = h.session.view();
  old.resolve('late success');
  assert.equal(await first, false);
  assert.deepEqual(h.session.view(), view);
  h.session.dispose();
});

test('nonempty release text including 仍在收尾 never counts as release success', async () => {
  const h = harness();
  await h.session.refresh();
  assert.equal(await h.session.start(args), true);
  h.state = running(1, 'request-1', 'stopping');
  assert.equal(await h.session.stop(), false);
  assert.equal(h.session.view().active, true);
  assert.equal(h.session.view().previewSession, null);
  assert.match(h.session.view().message, /仍在收尾/);
  h.state = idle();
  assert.equal(await h.session.refresh(), true);
  assert.equal(h.session.view().active, false);
  h.session.dispose();
});

test('disconnected state query preserves active/unknown and supports stop plus retry', async () => {
  const h = harness();
  await h.session.refresh();
  await h.session.start(args);
  h.handlers.set('render_state', () => { throw new Error('IPC disconnected'); });
  assert.equal(await h.session.refresh(), false);
  assert.equal(h.session.view().known, false);
  assert.equal(h.session.view().active, true);
  assert.equal(h.session.view().canStart, false);
  assert.equal(h.session.view().previewSession, null);
  assert.match(h.session.view().message, /未知/);
  assert.equal(await h.session.stop(), false);
  h.handlers.delete('render_state');
  h.state = idle();
  assert.equal(await h.session.refresh(), true);
  h.session.dispose();
});

test('failed cancellation acknowledgement cannot be bypassed by an idle sample', async () => {
  const h = harness();
  await h.session.refresh();
  await h.session.start(args);
  h.state = idle();
  h.handlers.set('render_cancel', () => { throw new Error('cancel unavailable'); });
  assert.equal(await h.session.stop(), false);
  assert.equal(h.session.view().canStart, false);
  assert.equal(h.session.view().active, true);
  h.handlers.delete('render_cancel');
  assert.equal(await h.session.stop(), true);
  h.session.dispose();
});

test('query deadlines are bounded without piling up raw IPC calls, and allow recovery', async () => {
  const h = harness();
  const pending = deferred();
  h.handlers.set('render_state', () => pending.promise);
  const first = h.session.refresh();
  await flush();
  h.clock.fire(3);
  await flush();
  assert.equal(await first, false);
  const second = h.session.refresh();
  await flush();
  h.clock.fire(3);
  await flush();
  assert.equal(await second, false);
  assert.equal(h.calls.filter((call) => call.command === 'render_state').length, 1);
  assert.equal(h.session.view().active, true);
  pending.resolve(idle());
  await flush();
  // A late query result after its deadline does not silently update UI.
  assert.equal(h.session.view().known, false);
  h.handlers.delete('render_state');
  assert.equal(await h.session.refresh(), true);
  h.session.dispose();
});

test('a state query begun before stop cannot restore the previous running session', async () => {
  const h = harness();
  await h.session.refresh();
  await h.session.start(args);
  const old = deferred();
  h.handlers.set('render_state', () => old.promise);
  const query = h.session.refresh();
  await flush();
  const stop = h.session.stop();
  await flush();
  old.resolve(running(1, 'request-1'));
  assert.equal(await query, false);
  assert.equal(await stop, false);
  assert.equal(h.session.view().previewSession, null);
  assert.equal(h.session.view().canStart, false);
  h.handlers.delete('render_state');
  h.state = idle();
  assert.equal(await h.session.refresh(), true);
  h.session.dispose();
});

test('missing device requests backend release before clearing UI; stopping remains active', async () => {
  const h = harness();
  await h.session.refresh();
  await h.session.start(args);
  const release = deferred();
  h.handlers.set('render_release', () => release.promise);
  h.state = running(1, 'request-1', 'stopping');
  let cleared = false;
  const cleanup = cleanupMissingDevice({
    stop: () => h.session.stop(), current: () => true,
    clear: () => {
      assert.ok(h.calls.some((call) => call.command === 'render_release'));
      cleared = true;
    },
  });
  await flush();
  assert.equal(cleared, false);
  release.resolve('仍在收尾');
  assert.equal(await cleanup, false);
  assert.equal(cleared, true);
  assert.equal(h.session.view().active, true);
  assert.equal(h.session.view().canStart, false);
  h.session.dispose();
});

test('preparation and device cleanup results obey intent tokens and disposal', async () => {
  const guard = createIntentGuard();
  const themes = deferred();
  const old = guard.begin();
  let selected = 'unchanged';
  const preparation = themes.promise.then(() => { if (guard.current(old)) selected = 'old themes'; });
  guard.invalidate(); // stop while loading themes / scanning
  const next = guard.begin();
  selected = 'new intent';
  themes.resolve();
  await preparation;
  assert.equal(selected, 'new intent');
  const stop = deferred();
  const cleanup = cleanupMissingDevice({ stop: () => stop.promise, current: () => guard.current(next),
    clear: () => { selected = 'cleared'; } });
  guard.dispose();
  stop.resolve(true);
  await cleanup;
  assert.equal(selected, 'new intent');
  assert.equal(guard.current(next), false);
});

function frame(id = 7n) {
  const bytes = new Uint8Array(24);
  bytes.set([74, 68, 80, 49]);
  new DataView(bytes.buffer).setBigUint64(4, id, true);
  bytes.set([137, 80, 78, 71, 13, 10, 26, 10], 12);
  return bytes.buffer;
}
test('JDP1 decoder validates magic, length, safe u64, PNG signature and current session', () => {
  assert.equal(decodePreviewFrame(frame(), 7)?.sessionId, 7);
  assert.equal(decodePreviewFrame(frame(), 8), null);
  assert.equal(decodePreviewFrame(frame(), null), null);
  assert.equal(decodePreviewFrame(frame(), Number.NaN), null);
  assert.equal(decodePreviewFrame(new ArrayBuffer(19), 7), null);
  assert.equal(decodePreviewFrame(frame(0n), 0), null);
  assert.equal(decodePreviewFrame(frame(9007199254740992n), 7), null);
  assert.equal(decodePreviewFrame(frame(18446744073709551615n), 7), null);
  const badMagic = frame(); new Uint8Array(badMagic)[0] = 0;
  assert.equal(decodePreviewFrame(badMagic, 7), null);
  const badPng = frame(); new Uint8Array(badPng)[12] = 0;
  assert.equal(decodePreviewFrame(badPng, 7), null);
  const max = Number.MAX_SAFE_INTEGER;
  assert.equal(decodePreviewFrame(frame(BigInt(max)), max)?.sessionId, max);
  // Legacy bare PNG must not pass as a session-framed preview.
  assert.equal(decodePreviewFrame(new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]).buffer, 7), null);
});

test('late bitmap after stop closes without painting or restoring hasPreview; no frame queue', async () => {
  let current = 7;
  let hasPreview = false;
  let decodeCount = 0;
  let closeCount = 0;
  const bitmap = deferred();
  const renderer = createPreviewRenderer({
    session: () => current,
    decode: () => { decodeCount++; return bitmap.promise; },
    paint: () => { hasPreview = true; }, clear: () => { hasPreview = false; },
  });
  const drawing = renderer.draw(frame());
  assert.equal(await renderer.draw(frame()), false);
  assert.equal(decodeCount, 1);
  current = null;
  renderer.invalidate();
  bitmap.resolve({ close: () => { closeCount++; } });
  assert.equal(await drawing, false);
  assert.equal(hasPreview, false);
  assert.equal(closeCount, 1);
  assert.equal(await renderer.draw(frame()), false);
  renderer.dispose();
});

test('old session frame and changed-session bitmap are discarded; valid current frame paints then closes', async () => {
  let current = 7;
  let paints = 0;
  let closes = 0;
  let decodeCount = 0;
  const pending = deferred();
  const renderer = createPreviewRenderer({
    session: () => current,
    decode: () => { decodeCount++; return decodeCount === 1 ? pending.promise : Promise.resolve({ close: () => closes++ }); },
    paint: () => { paints++; }, clear: () => {},
  });
  const old = renderer.draw(frame());
  current = 8; // even without explicit invalidate, post-decode session check is mandatory
  pending.resolve({ close: () => closes++ });
  assert.equal(await old, false);
  assert.equal(await renderer.draw(frame()), false);
  assert.equal(await renderer.draw(frame(8n)), true);
  assert.equal(paints, 1);
  assert.equal(closes, 2);
  renderer.dispose();
});

test('unmount ignores pending session results and closes pending bitmap without painting', async () => {
  const h = harness();
  const state = deferred();
  h.handlers.set('render_state', () => state.promise);
  const query = h.session.refresh();
  await flush();
  const before = h.views.length;
  h.session.dispose();
  state.resolve(idle());
  assert.equal(await query, false);
  assert.equal(h.views.length, before);
  const pending = deferred();
  let paints = 0, closes = 0;
  const renderer = createPreviewRenderer({ session: () => 7, decode: () => pending.promise,
    paint: () => paints++, clear: () => {} });
  const draw = renderer.draw(frame());
  renderer.dispose();
  pending.resolve({ close: () => closes++ });
  assert.equal(await draw, false);
  assert.equal(paints, 0);
  assert.equal(closes, 1);
});
