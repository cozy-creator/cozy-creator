const { sha } = require('./source-guards.cjs');
const originalSha = '196b0d9a6261b4d0e02a66135ac4b9f856eb9e66cc247fbdfb4b7f3eb58959f0';
const replacements = [
  ["const failures = [];", "const profileCapture = require('./profile-hooks.cjs');\nconst failures = [];"],
  ["const cdp = await context.newCDPSession(page);", "const cdp = await context.newCDPSession(page);\n  await profileCapture.bind(page, cdp);"],
  ["return frames.length ? summary(frames).p95_ms : Infinity;", "const p95 = frames.length ? summary(frames).p95_ms : Infinity;\n    profileCapture.frameWindow(frames, p95);\n    return p95;"],
  ["      pane.dispatchEvent(new WheelEvent('wheel', { deltaX: sign * 12, deltaY: 0, bubbles: true, cancelable: true }));", "      const observedBefore = window.__scaleRuntimeProfile?.read() ?? null;\n      pane.dispatchEvent(new WheelEvent('wheel', { deltaX: sign * 12, deltaY: 0, bubbles: true, cancelable: true }));\n      window.__scaleHarnessTelemetry?.frames.push({at_ms: now, frame_delta_ms: frames[frames.length-1], count, deltaX: sign * 12, before: observedBefore, after: window.__scaleRuntimeProfile?.read() ?? null});"],
  ["    const from = await worked();\n    const moving = summary(await panning(PAN_MS));\n    const to = await worked();", "    await profileCapture.startPan();\n    const from = await worked();\n    const moving = summary(await panning(PAN_MS));\n    const to = await worked();\n    await profileCapture.stopPan();"],
  ["  const opening = Date.now();", "  await profileCapture.startOpening();\n  const opening = Date.now();"],
  ["  check('it_settles_quickly', numbers.open_settled_s <= BARS.open_settled_s, numbers.open_settled_s);", "  check('it_settles_quickly', numbers.open_settled_s <= BARS.open_settled_s, numbers.open_settled_s);\n  await profileCapture.stopOpening();"],
  ["  await browser.close();", "  await profileCapture.finish();\n  await browser.close();\n  profileCapture.browserClosed();"],
  ["  await lastLook();", "  try { await profileCapture.finish(); } catch {}\n  await lastLook();"],
];
function instrument(original) {
  if (sha(original) !== originalSha) throw new Error('811 scale source guard failed');
  let text = original;
  for (const [from, to] of replacements) { if (text.split(from).length !== 2) throw new Error('harness injection is not unique'); text = text.replace(from, to); }
  let reversed = text;
  for (const [from, to] of [...replacements].reverse()) reversed = reversed.replace(to, from);
  if (reversed !== original) throw new Error('harness reversal failed');
  return { text, guard: { original_sha256: originalSha, instrumented_sha256: sha(text), reverse_exact: true, assertions_bars_fixture_unchanged: true } };
}
module.exports = { instrument, originalSha };
