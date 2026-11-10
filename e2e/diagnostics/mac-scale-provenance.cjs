const { execFileSync } = require('node:child_process');
const { readFileSync, writeFileSync } = require('node:fs');
const { createHash } = require('node:crypto');
const path = process.argv[2];
const info = JSON.parse(readFileSync(path, 'utf8'));
const command = (program, args) => execFileSync(program, args, { encoding: 'utf8' }).trim();
const binary = `${process.env.RUNNER_TEMP}/candidates/cozy`;
Object.assign(info, {
  purpose: 'Summary-only diagnosis; not required qualification or a gate replacement',
  artifact: { id: 13778929547, run: 42847320462, digest: 'sha256:68269b24926a7410563d8c4e35a3294854e1badea7e6e4d501e0dace52214bbb', inputs: '58de5a836886652eff2eea15c320ec04e95df59f', embedded_commit: 'fb74aa9de8bb0436415e52f093d191a955345243', same_tree_as_harness: 'edb8dff1886d5bb7ebe04e9c2875082ed519634a' },
  executable_sha256: createHash('sha256').update(readFileSync(binary)).digest('hex'),
  executable_version: command(binary, ['--version']),
  architecture: command('/usr/bin/file', [binary]),
  os: command('/usr/bin/sw_vers', []),
  runner_image: { os: process.env.ImageOS, version: process.env.ImageVersion },
  node: process.version,
  chrome: command('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--version']),
  differences: ['No preceding broad cli/web workload; the scale case internal warmup is retained.', 'Summary probes occur after original readings and may suspend the process/change between-phase timing.', 'All original scale assertions, RSS/heap/FD bars,5000 assets and10000ms close delay remain unchanged.', 'Logical Reader objects are not exposed by the existing binary; census covers all OS descriptors and VM-region metadata only.'],
});
if (!info.executable_version.includes('gfb74aa9d')) throw new Error('artifact has wrong candidate version');
writeFileSync(path, JSON.stringify(info, null, 2));
