// idle (coordinator, 11-17: if Chrome keeps compositing while the page sits idle, "every user's laptop burns CPU (and battery) with Cozy
// open, and GPU-less machines get starved like the runner"; the bar is zero frames composited while nothing changes): a project open and
// still, the pointer off the canvas and nothing focused. For 5 s no frame is drawn, no animation runs and nothing asks for an animation
// frame. A Windows runner's Chrome drew in software on every core it had while cells waited (e2e 44037098996: one process at 300% or
// more in 158 of 490 one-second samples, Calcifer and the Hub starved 10-20 s). Red, it says what kept frames coming: each running
// animation and what it's on, and where each animation frame was asked from.
const { test, expect, canvasIdle, canvasVisible, png, themed } = require('./fixtures');

const QUIET_MS = 5000;
const BOXES = ['first', 'second', 'third'];

/** Each requestAnimationFrame counted by where it was asked from: the two frames of its stack above this. */
function countAsked() {
  const asked = new Map();
  const request = window.requestAnimationFrame.bind(window);
  window.requestAnimationFrame = (callback) => {
    const at = (new Error().stack ?? '').split('\n').slice(2, 4).map((line) => line.trim().replace(/\?[^:)]*/, '')).join(' < ');
    asked.set(at, (asked.get(at) ?? 0) + 1);
    return request(callback);
  };
  window.__asked = () => {
    const said = [...asked].sort((a, b) => b[1] - a[1]);
    asked.clear();
    return said;
  };
}

/** The animations running on the page, each with what it's on. */
// A scroll's own (the fades at a list's ends, `animation-timeline: scroll()`) moves only with the scroll: only those on time count.
const running = (page) => page.evaluate(() => document.getAnimations().filter((animation) => animation.playState === 'running' && animation.timeline instanceof DocumentTimeline).map((animation) => {
  const target = animation.effect?.target;
  const on = target instanceof Element ? `${target.tagName.toLowerCase()}${target.id ? `#${target.id}` : ''}${[...target.classList].map((name) => `.${name}`).join('')}` : String(target ?? '');
  return `${animation.animationName ?? animation.transitionProperty ?? animation.constructor.name} on ${on}`;
}));

/** What Chrome did for `ms` with nothing changing: frames drawn, animation frames fired, paints. */
async function traced(page, ms) {
  const cdp = await page.context().newCDPSession(page);
  const events = [];
  cdp.on('Tracing.dataCollected', ({ value }) => events.push(...value));
  const complete = new Promise((done) => cdp.once('Tracing.tracingComplete', done));
  await cdp.send('Tracing.start', { traceConfig: { includedCategories: ['devtools.timeline', 'disabled-by-default-devtools.timeline.frame'] }, transferMode: 'ReportEvents' });
  await page.waitForTimeout(ms);
  await cdp.send('Tracing.end');
  await complete;
  await cdp.detach();
  const count = (name) => events.filter((event) => event.name === name).length;
  return { frames: count('DrawFrame'), animationFrames: count('FireAnimationFrame'), paints: count('Paint'), timers: count('TimerFire') };
}

test('idle', async ({ context, project, signIn }) => {
  await context.addInitScript(countAsked);
  for (const [i, name] of BOXES.entries()) await project.import(png(40 + i * 60), { name, at: { x: i * 300, y: 0 } });
  await project.add({ name: 'words', kind: 'text', fields: { text: 'Nothing moves.' }, at: { x: 0, y: 420 } });
  const page = await context.newPage();
  await signIn(page, project.url());

  for (const theme of ['paper', 'dark']) {
    await test.step(`${theme}: ${QUIET_MS / 1000} s still, no frame drawn, no animation, no animation frame asked`, async () => {
      await project.look(theme);
      await page.reload();
      await themed(page, theme);
      for (const name of [...BOXES, 'words']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
      await canvasIdle(page);
      await page.mouse.move(1, 1);
      await page.evaluate(() => (document.activeElement instanceof HTMLElement ? document.activeElement.blur() : undefined));
      await canvasIdle(page);
      // What it was still taking in (a picture landing, its note) done: a second with no animation frame asked.
      await expect.poll(async () => {
        await page.evaluate(() => window.__asked());
        await page.waitForTimeout(1000);
        return (await page.evaluate(() => window.__asked())).slice(0, 4);
      }, { message: `${theme}: the page settles, a second with no animation frame asked`, timeout: 20_000 }).toEqual([]);
      const did = await traced(page, QUIET_MS);
      const seen = { ...did, animations: await running(page), asked: (await page.evaluate(() => window.__asked())).slice(0, 8) };
      test.info().annotations.push({ type: 'note', description: `${theme}: ${JSON.stringify(seen)}` });
      expect.soft(seen, `${theme}: what Chrome did while nothing changed`).toMatchObject({ frames: 0, animationFrames: 0, animations: [], asked: [] });
    });
  }
});
