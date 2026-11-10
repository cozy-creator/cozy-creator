// Authentication-transparent transport. Same-origin admission is enforced before these functions.
function forwardHeaders(incoming, nativeBase) {
  const native = new URL(nativeBase);
  const headers = { ...incoming, host: native.host };
  if (headers.origin) headers.origin = native.origin;
  for (const key of Object.keys(headers)) if (key.startsWith('sec-fetch-')) delete headers[key];
  // Preserve both actual Authorization and Cookie. Never substitute the privileged holder bearer.
  return headers;
}
function upstreamUrl(target, nativeBase) {
  if (!target.startsWith('/v1/')) throw new Error('only relative native API targets are accepted');
  return new URL(target, nativeBase);
}
module.exports = { forwardHeaders, upstreamUrl };
