// select (cl-973 core cell 3), at zoom 1 and 0.25: a click selects a box and opens nothing; Shift adds one; a Shift-drag selects exactly
// what it touches; Ctrl+A selects all; a click on the canvas clears it; a hover never looks selected. A selected box has a ring of the
// selection's own colour, with its glow, 3 px or more on screen.
const { test, expect, canvasCard, canvasNames, canvasPoint, canvasIdle, selectedNames, hoverBox, openPoint, png, screenshotPixels, zoomOf, pointerOffCanvas } = require('./fixtures');

// Pictures 240 wide on the canvas, a row of three and one under them, clear of each other.
const PLACES = { A: { x: 0, y: 0 }, B: { x: 360, y: 0 }, C: { x: 720, y: 0 }, D: { x: 0, y: 380 } };
const NAMES = Object.keys(PLACES);

// The bare screenshot at this camera is the observed background for the glow, not a CSS approximation.
let baseline;
const near = (a, b, tolerance = 28) => a.every((value, i) => Math.abs(value - b[i]) <= tolerance);

/** Fractional native hover strokes are judged against the same native card with only its outline suppressed.
 * At 25%, the outline, paper border and picture can share one device pixel; adjacent pixels do not contain that underpaint.
 * Require visible paint toward the hover colour, not alpha equal to ideal rectangle area: transformed rasterization mixes underpaint.
 * The restored stroke must change a channel by at least 3 and stay within 8 RGB residual levels; selection has its separate 3 px/glow bar. */
function hoverCoverage(read, body, color, dpr, zoom = 1, withoutOutline = null) {
  const noise=8,width=zoom,offset=2*zoom;
  const sides=[['left',body.x,body.y+body.height/2,1,true],['right',body.x+body.width,body.y+body.height/2,-1,true],
    ['top',body.y,body.x+body.width/2,1,false],['bottom',body.y+body.height,body.x+body.width/2,-1,false]];
  return sides.map(([side,edge,across,inward,horizontal])=>{
    const [low,high]=[(edge-inward*(offset+width))*dpr,(edge-inward*offset)*dpr].sort((a,b)=>a-b);
    const start=Math.floor(low),end=Math.ceil(high)-1,outside=inward===1?start-1:end+1;
    const pixel=coordinate=>horizontal?read(coordinate,Math.round(across*dpr)):read(Math.round(across*dpr),coordinate);
    const near=pixel(outside),far=pixel(outside-inward),samples=[];
    let visible=false;
    for(let coordinate=start;coordinate<=end;coordinate++) {
      const observed=pixel(coordinate),steps=(coordinate-outside)*inward;
      const background=withoutOutline ? (horizontal ? withoutOutline(coordinate,Math.round(across*dpr)) : withoutOutline(Math.round(across*dpr),coordinate)) : near.map((value,c)=>value+(value-far[c])*steps),toward=color.map((value,c)=>value-background[c]);
      const square=toward.reduce((sum,value)=>sum+value*value,0);
      const alpha=square?observed.reduce((sum,value,c)=>sum+(value-background[c])*toward[c],0)/square:NaN;
      const expected=Math.min(coordinate+1,high)-Math.max(coordinate,low);
      const residual=Math.max(...observed.map((value,c)=>Math.abs(value-background[c]-alpha*toward[c])));
      const uncertainty=square?noise*toward.reduce((sum,value)=>sum+Math.abs(value),0)/square:Infinity;
      const delta=Math.max(...observed.map((value,c)=>Math.abs(value-background[c])));
      const valid=delta<3 || (Number.isFinite(alpha) && alpha>0 && alpha<=1+uncertainty && residual<=noise && background.every(value=>value>=0 && value<=255));
      if(delta>=3)visible=true;
      samples.push({coordinate,observed,background,delta,geometricArea:expected,coverage:Number.isFinite(alpha)?alpha:null,residual:Number.isFinite(residual)?residual:null,uncertainty:Number.isFinite(uncertainty)?uncertainty:null,valid});
    }
    const valid=visible && samples.length>0 && samples.every(sample=>sample.valid);
    const projectedAlphaSum=samples.every(sample=>sample.coverage!==null)?samples.reduce((sum,sample)=>sum+sample.coverage,0):null;
    return {side,valid,projectedAlphaSum,nominalStrokeWidth:width,outlineOffset:offset,noise,backgroundSamples:[near,far],samples};
  });
}

/** Four rays through real screenshot pixels: the solid selection stroke and its translucent outer glow. */
function edge(read, body, color, dpr) {
  const rays = [t => [body.x + t, body.y + body.height / 2], t => [body.x + body.width - t, body.y + body.height / 2],
    t => [body.x + body.width / 2, body.y + t], t => [body.x + body.width / 2, body.y + body.height - t]];
  return rays.map(point => {
    const at = t => { const [x, y] = point(t); return read(x * dpr, y * dpr); };
    let width = 0, run = 0, first = null;
    for (let t = -20; t <= 20; t += 0.5) {
      if (near(at(t), color)) { run += 0.5; if (first === null) first = t; width = Math.max(width, run); }
      else run = 0;
    }
    let glow = false;
    if (baseline && first !== null) for (let t = first - 3; t >= first - 11; t--) {
      const [x, y] = point(t), base = baseline(x * dpr, y * dpr), drawn = at(t);
      const toward = color.map((value, i) => value - base[i]), change = drawn.map((value, i) => value - base[i]);
      const size = toward.reduce((sum, value) => sum + value * value, 0);
      const alpha = change.reduce((sum, value, i) => sum + value * toward[i], 0) / size;
      if (alpha > 0.015 && alpha < 0.65 && change.some(value => Math.abs(value) >= 3)
        && near(drawn, base.map((value, i) => value + alpha * toward[i]), 8)) glow = true;
    }
    return { width, glow };
  });
}

async function rings(page) {
  const state = await page.evaluate(names => {
    const inspection = document.querySelector('.canvas-root').canvasInspection;
    const probe = document.createElement('i'); document.body.append(probe);
    const color = token => {
      probe.style.color = `var(${token})`;
      const css = getComputedStyle(probe).color, values = css.match(/[\d.]+/g).map(Number).slice(0, 3);
      return css.startsWith('color(') ? values.map(value => value * 255) : values;
    };
    const result = { select: color('--select'), hover: color('--line2'), dpr: devicePixelRatio, zoom: inspection.camera().zoom,
      bodies: Object.fromEntries(names.map(name => [name, inspection.card(name)?.body])) };
    const native = [...document.querySelectorAll('.canvas-node .card')].find(card => card.matches(':hover'));
    const media = native?.querySelector(':scope > .media');
    if (media) {
      const r = media.getBoundingClientRect(), style = getComputedStyle(media);
      result.native = { name: native.dataset.asset, shadow: style.boxShadow, body: { x: r.x, y: r.y, width: r.width, height: r.height },
        outline: { style: style.outlineStyle, width: style.outlineWidth, color: style.outlineColor, offset: style.outlineOffset } };
    }
    probe.remove(); return result;
  }, NAMES);
  let withoutOutline = null, artifacts;
  if (state.native) {
    const css = state.native.outline.color, values = css.match(/[\d.]+/g).map(Number).slice(0, 3);
    expect(state.native.outline.style, 'hover has a solid native outline').toBe('solid');
    expect(css.startsWith('color(') ? values.map(value => value * 255) : values, 'the native hover outline uses --line2').toEqual(state.hover);
    const media = page.locator(`.canvas-node .card[data-asset=${JSON.stringify(state.native.name)}] > .media`).first();
    artifacts = { withoutOutline: test.info().outputPath(`hover-${state.zoom}-without-outline.png`), restored: test.info().outputPath(`hover-${state.zoom}-restored.png`) };
    const inline = await media.evaluate(el => {
      const held = { value: el.style.getPropertyValue('outline-style'), priority: el.style.getPropertyPriority('outline-style') };
      el.style.setProperty('outline-style', 'none', 'important');
      return held;
    });
    try {
      const control = await media.evaluate(el => { const r = el.getBoundingClientRect(), style = getComputedStyle(el); return { style: style.outlineStyle, shadow: style.boxShadow, body: { x: r.x, y: r.y, width: r.width, height: r.height } }; });
      expect(control, 'only the outline is suppressed; native geometry and shadow stay intact').toEqual({ style: 'none', shadow: state.native.shadow, body: state.native.body });
      withoutOutline = screenshotPixels(await page.screenshot({ path: artifacts.withoutOutline }));
    } finally {
      await media.evaluate((el, held) => {
        if (held.value) el.style.setProperty('outline-style', held.value, held.priority);
        else el.style.removeProperty('outline-style');
      }, inline);
    }
    const restored = await media.evaluate(el => {
      const r = el.getBoundingClientRect(), style = getComputedStyle(el);
      return { shadow: style.boxShadow, body: { x: r.x, y: r.y, width: r.width, height: r.height },
        outline: { style: style.outlineStyle, width: style.outlineWidth, color: style.outlineColor, offset: style.outlineOffset } };
    });
    expect(restored, 'the native hover outline and geometry are restored after its negative control').toEqual({ shadow: state.native.shadow, body: state.native.body, outline: state.native.outline });
  }
  const read = screenshotPixels(await page.screenshot(artifacts ? { path: artifacts.restored } : {}));
  return Object.fromEntries(NAMES.map(name => {
    const body = state.bodies[name];
    if (!body) return [name, null];
    const sides = edge(read, body, state.select, state.dpr);
    const native = state.native?.name === name ? state.native : null;
    const hovered = hoverCoverage(read, native?.body ?? body, state.hover, state.dpr, state.zoom, native ? withoutOutline : null);
    const px = Math.min(...sides.map(side => side.width));
    return [name, { solid: sides.every(side => side.width >= 3), look: px >= 3 ? 'selection' : hovered.every(side => side.valid) ? 'hover' : 'neither',
      glow: sides.filter(side => side.glow).length >= 2, px, sides, hovered, ...(native ? { native, artifacts, zoom: state.zoom, dpr: state.dpr } : {}) }];
  }));
}

async function passive(page) {
  await pointerOffCanvas(page);
  await expect(page.locator('.canvas-node')).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => document.querySelector('.canvas-root')?.canvasInspection?.resources()?.scheduledFrame), { message: 'the passive scene is presented and idle' }).toBe(false);
}

async function rememberBare(page) {
  await passive(page);
  await expect.poll(async () => {
    const read = screenshotPixels(await page.screenshot()), dpr = await page.evaluate(() => devicePixelRatio);
    for (const [i, name] of NAMES.entries()) {
      const body = (await canvasCard(page, name))?.rendered?.body;
      if (!body || !near(read((body.x + body.width / 2) * dpr, (body.y + body.height / 2) * dpr), [40 + 50 * i, 127, 127], 12)) return false;
    }
    baseline = read;
    return true;
  }, { message: 'all four real pictures painted before their rings are measured' }).toBe(true);
}

async function ringed(page, names) {
  await passive(page);
  let all;
  await expect.poll(async () => {
    all = await rings(page);
    return NAMES.every(name => Boolean(all[name]) && (names.includes(name) ? all[name].solid && all[name].glow : all[name].look !== 'selection' && !all[name].glow));
  }, { message: `the painted rings select ${names.join(', ') || 'nothing'}` }).toBe(true);
  for (const name of NAMES) {
    const said = all[name];
    if (names.includes(name)) {
      expect.soft(said, `${name} ringed in the selection's colour, with its glow`).toMatchObject({ solid: true, look: 'selection', glow: true });
      expect.soft(said.px, `${name}'s ring on screen, px`).toBeGreaterThanOrEqual(3);
    } else expect.soft(said, `${name} not ringed`).toMatchObject({ look: expect.not.stringMatching(/^selection$/), glow: false });
  }
}

async function click(page, name, modifier) {
  const { x, y } = await canvasPoint(page, name);
  if (modifier) await page.keyboard.down(modifier);
  await page.mouse.click(x, y);
  if (modifier) await page.keyboard.up(modifier);
}

/** The zoom menu's `items` in turn, then Zoom out `out` times: the canvas still at `percent`. */
async function zoomTo(page, items, out, percent) {
  for (const item of items) {
    await page.getByRole('button', { name: /more zoom options$/ }).click();
    await page.getByRole('menuitem', { name: new RegExp(`^${item}`) }).click();
    await canvasIdle(page);
  }
  for (let i = 0; i < out; i++) await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
  await canvasIdle(page);
  await expect(page.getByRole('button', { name: /more zoom options$/ })).toHaveText(percent);
}

test('select', async ({ page, project }) => {
  for (const [i, name] of NAMES.entries()) await project.import(png(40 + 50 * i), { name, at: PLACES[name] });
  await expect.poll(() => canvasNames(page)).toEqual(expect.arrayContaining(NAMES));
  const url = page.url();

  // All four in view, then at 100% about the middle; then 50% and two steps out.
  for (const [zoom, items, out, percent] of [[1, ['Zoom to fit', 'Zoom to 100%'], 0, '100%'], [0.25, ['Zoom to 50%'], 2, '25%']]) {
    await test.step(`at zoom ${zoom}`, async () => {
      await zoomTo(page, items, out, percent);
      expect(await zoomOf(page)).toBeCloseTo(zoom, 3);
      await expect.poll(() => selectedNames(page)).toEqual([]);
      await rememberBare(page);

      await test.step('a click selects A and opens nothing', async () => {
        await click(page, 'A');
        await expect.poll(() => selectedNames(page)).toEqual(['A']);
        await expect.soft(page.getByRole('region', { name: 'Focused asset' })).toHaveCount(0);
        expect.soft(page.url(), 'the same address').toBe(url);
        await ringed(page, ['A']);
      });

      await test.step('Shift adds B', async () => {
        await click(page, 'B', 'Shift');
        await expect.poll(() => selectedNames(page)).toEqual(['A', 'B']);
        await ringed(page, ['A', 'B']);
      });

      await test.step('Esc clears it', async () => {
        await page.keyboard.press('Escape');
        await expect.poll(() => selectedNames(page)).toEqual([]);
        await ringed(page, []);
      });

      await test.step('a Shift-drag selects what it touches: A, B and C', async () => {
        const [a, c] = [(await canvasCard(page, 'A')).body, (await canvasCard(page, 'C')).body];
        const from = { x: a.x - 12, y: a.y + a.height / 4 };
        const to = { x: c.x + c.width / 2, y: c.y + (c.height * 3) / 4 };
        expect(await page.evaluate(({ x, y }) => Boolean(document.querySelector('.canvas-root')?.contains(document.elementFromPoint(x, y)) && !document.querySelector('.canvas-root').canvasInspection.hitTest({ x, y })), from), 'the drag starts on open canvas').toBe(true);
        await page.keyboard.down('Shift');
        await page.mouse.move(from.x, from.y);
        await page.mouse.down();
        await page.mouse.move((from.x + to.x) / 2, (from.y + to.y) / 2, { steps: 4 });
        await page.mouse.move(to.x, to.y, { steps: 4 });
        await page.mouse.up();
        await page.keyboard.up('Shift');
        await expect.poll(() => selectedNames(page)).toEqual(['A', 'B', 'C']);
        await ringed(page, ['A', 'B', 'C']);
      });

      await test.step('Ctrl+A selects all', async () => {
        await page.keyboard.press('ControlOrMeta+a');
        await expect.poll(() => selectedNames(page)).toEqual(NAMES);
        await ringed(page, NAMES);
      });

      await test.step('a click on the canvas clears it', async () => {
        const { x, y } = await openPoint(page);
        await page.mouse.click(x, y);
        await expect.poll(() => selectedNames(page)).toEqual([]);
        await ringed(page, []);
      });

      await test.step('a hover never looks selected', async () => {
        await hoverBox(page, 'D');
        let hovered;
        try {
          await expect.poll(async () => { hovered = (await rings(page)).D; return hovered.look; }, 'D looks hovered').toBe('hover');
        } finally {
          if (hovered) {
            await test.info().attach(`hover-${zoom}-rays`, { body: Buffer.from(JSON.stringify(hovered, null, 2)), contentType: 'application/json' });
            for (const [kind, path] of Object.entries(hovered.artifacts ?? {})) await test.info().attach(`hover-${zoom}-${kind}`, { path, contentType: 'image/png' });
          }
        }
        expect.soft(hovered, 'a hover has no glow').toMatchObject({ glow: false });
        expect.soft(await selectedNames(page)).not.toContain('D');
        await pointerOffCanvas(page);
      });
    });
  }
});
