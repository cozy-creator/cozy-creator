// A selected box is obvious on the canvas (cl-926, Paul 11-12): a thick ring of its own colour outside it, with a gap and a glow, at least 3px
// on screen at zoom 0.25 and 1, on paper and dark; several selected all show it; hover and a check's green look otherwise; the box-select
// rectangle shows. In a real browser on a real Calcifer, with screenshots of each.
//   node selected.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFolder, boxOf, calcifer, fitCanvas, lastLook, launch, lookAt, noStall, png, projectFromPage, settled, signIn, STEP_MS, until, upload } = require('./page');

const { canvasBox, canvasView, selectedBoxes, screenshotPixels } = require('./page');

const failures = [];
const seen = {};
if (require.main === module) noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(why) {
  failures.push(`${during}: ${why}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}

const NAMES = ['light', 'dark', 'middle'];

/** Diagnostics only. Keep the hover verdict's original 28-channel threshold and width requirements unchanged. */
async function hoverDiagnostics(page, name = 'light', path) {
  const state=await page.evaluate(name=>{
    const root=document.querySelector('.canvas-root'), inspection=root?.canvasInspection;
    const native=[...document.querySelectorAll('.canvas-node')].find(node=>node.querySelector('.card')?.dataset.asset===name);
    const card=native?.querySelector('.card'),media=card?.querySelector(':scope > .media');
    const rect=element=>{if(!element)return null;const b=element.getBoundingClientRect();return {x:b.x,y:b.y,width:b.width,height:b.height};};
    const style=element=>{if(!element)return null;const s=getComputedStyle(element);return {outlineStyle:s.outlineStyle,outlineWidth:s.outlineWidth,outlineOffset:s.outlineOffset,outlineColor:s.outlineColor,boxShadow:s.boxShadow,opacity:s.opacity,visibility:s.visibility,transform:s.transform,borderTopWidth:s.borderTopWidth,borderLeftWidth:s.borderLeftWidth};};
    const probe=document.createElement('span');probe.style.color='var(--line2)';document.body.append(probe);const line=getComputedStyle(probe).color;probe.remove();
    const rgb=line.match(/[\d.]+/g).map(Number).slice(0,3),expected=line.startsWith('color(')?rgb.map(value=>value*255):rgb;
    const body=inspection?.card(name)?.body ?? null;
    const under=body?document.elementFromPoint(body.x+body.width/2,body.y+body.height/2):null;
    return {name,theme:document.documentElement.dataset.theme ?? 'paper',dpr:devicePixelRatio,camera:inspection?.camera(),body,expected,line,
      native:{present:Boolean(native),selected:native?.classList.contains('selected'),node:rect(native),card:rect(card),media:rect(media),mediaStyle:style(media),cardStyle:style(card),nodeStyle:style(native),worldStyle:style(native?.closest('.canvas-world')),
        hovered:{node:native?.matches(':hover'),card:card?.matches(':hover'),media:media?.matches(':hover')},images:[...media?.querySelectorAll('img') ?? []].map(image=>({complete:image.complete,width:image.naturalWidth,height:image.naturalHeight}))},
      underBodyCenter:under?{tag:under.tagName,className:String(under.className),asset:under.closest('.card')?.dataset.asset}:null};
  },name);
  const bytes=await page.screenshot(path?{path}:{}),read=screenshotPixels(bytes);
  const coverage=body=>{
    if(!body)return null;
    const rays=[['left',t=>[body.x+t,body.y+body.height/2]],['right',t=>[body.x+body.width-t,body.y+body.height/2]],['top',t=>[body.x+body.width/2,body.y+t]],['bottom',t=>[body.x+body.width/2,body.y+body.height-t]]];
    return rays.map(([side,point])=>{
      let width=0,run=0,minimumDistance=Infinity,accepted=0;
      const samples=[];
      for(let offset=-20;offset<=20;offset+=0.5){
        const [x,y]=point(offset),px=Math.round(x*state.dpr),py=Math.round(y*state.dpr);
        if(px<0 || py<0 || px>=read.width || py>=read.height){samples.push([offset,px,py,null]);run=0;continue;}
        const rgb=read(px,py),distance=Math.max(...rgb.map((value,c)=>Math.abs(value-state.expected[c])));
        minimumDistance=Math.min(minimumDistance,distance);samples.push([offset,px,py,...rgb]);
        if(distance<=28){accepted++;run+=0.5;width=Math.max(width,run);}else run=0;
      }
      return {side,width,accepted,minimumDistance:Number.isFinite(minimumDistance)?minimumDistance:null,samples};
    });
  };
  return {...state,screenshot:path ?? null,inspectionRays:coverage(state.body),nativeRays:coverage(state.native.media)};
}

async function main() {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-selected-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const canvas = page.url();
  const { id } = await projectFromPage(page);
  const session = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = async (path, method = 'GET', body) => {
    const answer = await fetch(base + path, {
      method,
      headers: { Authorization: `Bearer ${session.token}`, 'cozy-project-storage': '3', 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const value = await answer.json().catch(() => null);
    if (!answer.ok) throw new Error(`${method} ${path}: ${answer.status} ${JSON.stringify(value)}`);
    return value;
  };
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).path };

  // A near-white picture, a near-black one and one between: the ring has to read against each.
  const pictures = mkdtempSync(join(tmpdir(), 'web-selected-'));
  for (const [i, name] of NAMES.entries()) {
    const path = join(pictures, `${name}.png`);
    const grey = [250, 8, 128][i];
    writeFileSync(path, png(0, 320, 200, false, () => [grey, grey, grey]));
    await api(`${at}/imports`, 'POST', { source: await upload(session.token, path, id), name, exact: true, at: { x: i * 300, y: 0 } });
  }

  const zoomOf = async () => (await canvasView(page)).zoom;
  const zoomTo = async (wanted) => {
    await page.mouse.move(2, 500);
    await page.keyboard.press('ControlOrMeta+0');
    await settled(page);
    for (let i = 0; i < 12 && (await zoomOf()) > wanted + 0.001; i++) {
      await page.keyboard.press('-');
      await settled(page);
    }
    return zoomOf();
  };
  const middle = async (name) => {
    const { x, y, width, height } = (await canvasBox(page, name)).body;
    return { x: x + width / 2, y: y + height / 2 };
  };
  const colors = () => page.evaluate(() => {
    const probe = document.createElement('span'); document.body.append(probe);
    const rgb = (token) => {
      probe.style.color = `var(${token})`;
      const color = getComputedStyle(probe).color;
      const values = color.match(/[\d.]+/g).map(Number).slice(0, 3);
      return color.startsWith('color(') ? values.map(value => value * 255) : values;
    };
    const result = { select: rgb('--select'), cur: rgb('--cur'), line: rgb('--line2') };
    probe.remove(); return result;
  });
  let baseline;
  const near = (a, b, tolerance = 28) => a.every((v,i) => Math.abs(v-b[i]) <= tolerance);
  /** Read full-colour strokes along four rays through actual screenshot pixels, in CSS pixels. */
  const edge = (read, body, color, dpr) => {
    const ray = [t => [body.x+t, body.y+body.height/2], t => [body.x+body.width-t, body.y+body.height/2],
      t => [body.x+body.width/2, body.y+t], t => [body.x+body.width/2, body.y+body.height-t]];
    return ray.map(point => {
      const at = t => { const [x,y] = point(t); return read(x*dpr,y*dpr); };
      let width = 0, run = 0, first = null;
      for (let t=-20; t<=20; t+=0.5) {
        if (near(at(t), color)) { run+=0.5; if (first === null) first=t; width=Math.max(width,run); } else run=0;
      }
      let glow = false;
      if (baseline && first !== null) for (let t=first-3; t>=first-11; t-=1) {
        const [x,y] = point(t), base = baseline(x*dpr,y*dpr), drawn=at(t);
        const toward=color.map((v,i)=>v-base[i]), change=drawn.map((v,i)=>v-base[i]);
        const size=toward.reduce((sum,v)=>sum+v*v,0), alpha=change.reduce((sum,v,i)=>sum+v*toward[i],0)/size;
        if (alpha>0.015 && alpha<0.65 && change.some(v=>Math.abs(v)>=3) && near(drawn,base.map((v,i)=>v+alpha*toward[i]),8)) glow=true;
      }
      return { width, glow };
    });
  };
  const rings = async () => {
    const palette = await colors(), selected = await selectedBoxes(page), dpr = await page.evaluate(() => devicePixelRatio);
    const read=screenshotPixels(await page.screenshot()), out={ ...palette, zoom:await zoomOf(), boxes:{} };
    for (const name of NAMES) {
      const body=(await canvasBox(page,name)).body, sides=edge(read,body,palette.select,dpr), hovered=edge(read,body,palette.line,dpr);
      out.boxes[name]={selected:selected.includes(name),width:Math.min(...sides.map(side=>side.width)),glow:sides.filter(side=>side.glow).length>=2,hoverWidth:Math.min(...hovered.map(side=>side.width)),hoverSides:hovered,sides};
    }
    return out;
  };
  const ringsWhen = (ok) => until('the displayed rings as wanted', rings, ok, 5000).catch(rings);
  const ringed = box => box?.selected && box.width >= 3 - 0.05 && box.glow;
  const bare = box => box && !box.selected && box.width < 0.5;
  const rememberBare = async () => {
    await page.mouse.move(2,500);
    await until('native overlay off',()=>page.locator('.canvas-node').count(),n=>n===0);
    baseline=await until('three real pictures drawn',async()=> {
      const read=screenshotPixels(await page.screenshot()),dpr=await page.evaluate(()=>devicePixelRatio);
      for (const [i,name] of NAMES.entries()) { const b=(await canvasBox(page,name)).body; if (!near(read((b.x+b.width/2)*dpr,(b.y+b.height/2)*dpr),[[250,250,250],[8,8,8],[128,128,128]][i],12)) return null; }
      return read;
    },Boolean);
  };

  for (const theme of ['paper', 'dark']) {
    await page.evaluate((theme) => localStorage.setItem('cozy.theme', theme), theme);
    await page.goto(canvas);
    for (const name of NAMES) await until(`${name} on the canvas`, () => canvasBox(page,name), Boolean);
    check(`${theme}_theme_shown`, (await page.evaluate(() => document.documentElement.dataset.theme ?? 'paper')) === theme);
    await fitCanvas(page);
    for (const zoom of [1, 0.25]) {
      const tag = `${theme}_${zoom}`;
      seen[`${tag}_zoom`] = await zoomTo(zoom);
      // At the zoom it says: a ring judged at 1 when 0.25 was asked proves nothing of 0.25.
      check(`${tag}_at_its_zoom`, Math.abs(seen[`${tag}_zoom`] - zoom) < 0.01, seen[`${tag}_zoom`]);
      await page.keyboard.press('Escape');
      await until('selection cleared', () => selectedBoxes(page), names => names.length === 0);
      await rememberBare();
      // One, by a click.
      const point = await middle('middle');
      await page.mouse.click(point.x, point.y);
      await page.mouse.move(2, 500);
      const one = (now) => ringed(now.boxes.middle, now) && bare(now.boxes.light, now) && bare(now.boxes.dark, now);
      let look = await ringsWhen(one);
      check(`${tag}_one_ringed`, one(look), look);
      await shot(`${theme}-${zoom}-one`);
      // Several: all of them.
      await page.keyboard.press('ControlOrMeta+a');
      const all = (now) => NAMES.every((name) => ringed(now.boxes[name], now));
      look = await ringsWhen(all);
      check(`${tag}_several_ringed`, all(look), look);
      await shot(`${theme}-${zoom}-several`);
      await page.keyboard.press('Escape');
      await ringsWhen((now) => NAMES.every((name) => !now.boxes[name]?.selected));
    }
    // Hover and a check's green aren't it.
    await zoomTo(1);
    const point = await middle('light');
    await page.mouse.move(point.x, point.y);
    const look = await ringsWhen((now) => now.boxes.light?.hoverWidth > 0);
    seen[`${theme}_hover_diagnostics`]=await hoverDiagnostics(page,'light',process.env.SHOT?`${process.env.SHOT}-${theme}-hover.png`:undefined);
    check(`${theme}_hover_is_not_selection`, bare(look.boxes.light, look) && look.boxes.light.hoverWidth > 0 && look.boxes.light.hoverWidth < 3, look.boxes.light);
    check(`${theme}_selection_is_not_the_check_green`, JSON.stringify(look.select) !== JSON.stringify(look.cur) && JSON.stringify(look.select) !== JSON.stringify(look.line), { select: look.select, cur: look.cur, line: look.line });
    await page.mouse.move(2, 500);

    // The box-select rectangle, while it's drawn: Shift and a drag from open canvas over all three.
    await fitCanvas(page);
    await rememberBare();
    const first = (await canvasBox(page, 'light')).body;
    const last = (await canvasBox(page, 'middle')).body;
    const from = { x: first.x - 40, y: first.y - 60 };
    const to = { x: last.x + last.width + 40, y: last.y + last.height + 60 };
    const beforeMarquee=screenshotPixels(await page.screenshot());
    await page.keyboard.down('Shift');
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move((from.x + to.x) / 2, (from.y + to.y) / 2, { steps: 4 });
    await page.mouse.move(to.x, to.y, { steps: 4 });
    const rect = await page.locator('.canvas-selection:not([hidden])').boundingBox();
    const dpr=await page.evaluate(()=>devicePixelRatio), drawn=screenshotPixels(await page.screenshot()), palette=await colors();
    const border=rect ? edge(drawn,rect,palette.select,dpr) : [];
    const filled=rect ? drawn((rect.x+8)*dpr,(rect.y+8)*dpr) : null;
    await shot(`${theme}-box-select`);
    await page.mouse.up();
    await page.keyboard.up('Shift');
    check(`${theme}_box_select_rectangle_shows`, Boolean(rect) && border.every(side=>side.width>=1.5) && filled !== null && filled.some((value,i)=>Math.abs(value-beforeMarquee((rect.x+8)*dpr,(rect.y+8)*dpr)[i])>=3), rect);
    const after = await ringsWhen((now) => NAMES.every((name) => ringed(now.boxes[name], now)));
    check(`${theme}_box_select_rings_all`, NAMES.every((name) => ringed(after.boxes[name], after)), after);
    await shot(`${theme}-box-selected`);
    await page.keyboard.press('Escape');
  }
  await page.evaluate(() => localStorage.removeItem('cozy.theme'));

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
}

if (require.main === module) main().catch((why) => died(why.stack ?? why));
module.exports = { hoverDiagnostics };
