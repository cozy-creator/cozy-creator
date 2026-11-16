// Chrome's launch flags on this runner, for playwright.config.js and the cells that set their own (cl-973).
// A runner has no GPU but a Mac's. Linux's Chrome draws WebGL on SwiftShader with no major performance caveat; Windows' Chrome flags it,
// so there ANGLE takes SwiftShader as its device (as layout.spec.js does everywhere).
const SOFTWARE = { linux: ['--enable-unsafe-swiftshader'], win32: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader'] };
const chromeArgs = (...more) => [...(SOFTWARE[process.platform] ?? []), ...more];

module.exports = { chromeArgs };
