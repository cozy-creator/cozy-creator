// This runner's speed against the fast hosted runners, for time bars set on those (cl-870). Runners of one label differ
// ~1.5x: web-scale's fixture took 0.03 s on some and 0.05 s on others, and every timed cell moved with it (cl-869:
// e2e 43012020357's same-runner A/B cleared the Hub and Calcifer). The work is fixed and touches no cozy: a 5,000-asset
// project's records built, written as JSON, read back and sorted, the median of 15 after 3 to warm up.
const { cpus } = require('os');

/** The fast runners' median, whose fixture took 0.03 s, where the bars were set: AMD EPYC 9V74, 16.6 ms at the start and 17.6 at
 *  the end (e2e 43019563028). An EPYC 9V45 reads 12.7-13.0 (fixture 0.02 s, e2e 43017866820, 43019615168) and keeps the bars too. */
const REFERENCE_MS = 17;
const REPS = 15;

function work() {
  const words = 'The colossal blue whale rises through a storm, lit from below, the sea torn white around it, and the small boat holds. '.repeat(3);
  const assets = [];
  for (let i = 0; i < 5000; i++) {
    const versions = [1, 2].map((number) => ({ number, seed: i * number, run: 1000 + i * 2 + number, files: { image: `sha256:${((i * 2654435761 + number) >>> 0).toString(16).padStart(64, '0')}` } }));
    assets.push({ name: `shot_${i}`, standing: 'current', fields: { instructions: `${words}<pic_${i % 1000}> and <pic_${(i * 7) % 1000}>.`, aspect_ratio: '16:9' }, versions });
  }
  const back = JSON.parse(JSON.stringify({ assets })).assets;
  back.sort((a, b) => b.versions[1].seed - a.versions[1].seed || (a.name < b.name ? -1 : 1));
  return back.length;
}

/** { cpu, cores, median_ms, reference_ms, factor, reps_ms }: `factor` is what a time bar is multiplied by, never under 1. */
function runnerSpeed() {
  for (let i = 0; i < 3; i++) work();
  const took = [];
  for (let i = 0; i < REPS; i++) {
    const began = performance.now();
    work();
    took.push(performance.now() - began);
  }
  const sorted = [...took].sort((a, b) => a - b);
  const median = sorted[REPS >> 1];
  return {
    cpu: cpus()[0]?.model ?? 'unknown',
    cores: cpus().length,
    median_ms: +median.toFixed(2),
    reference_ms: REFERENCE_MS,
    factor: +Math.max(1, median / REFERENCE_MS).toFixed(2),
    reps_ms: took.map((ms) => +ms.toFixed(1)),
  };
}

module.exports = { runnerSpeed };
