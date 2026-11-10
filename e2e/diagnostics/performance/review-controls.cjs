const assert = require('node:assert/strict');
const vm = require('node:vm');
const attribution = require('./attribution.cjs');
for (const value of [undefined, null, false, 'true', 1, {}]) assert.match(attribution(value), /^NO regression\/fix attribution:/);
assert.match(attribution(true), /^pending matched analysis;/);
process.env.PROFILE_CASE = 'current-plain';
process.env.PROFILE_CASE_OUTPUT = '/unused-pure-control';
const { bind } = require('./profile-hooks.cjs');
const realm = { window: {}, performance: { timeOrigin: 123 } };
const future = [];
(async () => {
  await bind({
    addInitScript: async fn => future.push(fn),
    evaluate: async fn => vm.runInNewContext(`(${fn.toString()})()`, realm),
    on() {},
  }, { on() {} });
  assert.equal(realm.window.__scaleHarnessTelemetry.phase, 'boot');
  assert.equal(realm.window.__scaleHarnessTelemetry.frames.length, 0);
  assert.equal(realm.window.__scaleHarnessTelemetry.time_origin_ms, 123);
  assert.equal(future.length, 1);
  const nextDocument = { window: {}, performance: { timeOrigin: 456 } };
  vm.runInNewContext(`(${future[0].toString()})()`, nextDocument);
  assert.equal(nextDocument.window.__scaleHarnessTelemetry.time_origin_ms, 456);
  console.log(JSON.stringify({ current_page_and_future_document_initialized: true, unknown_attribution_forbidden: true, no_browser_native_or_port: true }));
})().catch(error => { console.error(error.message); process.exitCode = 1; });
