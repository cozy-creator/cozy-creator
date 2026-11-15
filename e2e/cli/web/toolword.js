// A box on the canvas says what tool makes it (cl-927, Paul 11-12: "in the canvas free-roam mode, it should display what tools are attached
// to a node ... I want to see what tools are attached"), in a real browser on a real Calcifer. Under its picture (cl-932), a picture with Qwen's Image
// and a video with H3's Shot say the package and the tool ("Qwen Image 2 · Image"), readable, and shrinking with the box as the canvas zooms
// out; a picture and words with no tool say nothing there. A screenshot of them.
//   node toolword.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFolder, projectFromPage, calcifer, signIn, MP4, STEP_MS, boxOf, fitCanvas, lastLook, launch, lookAt, noStall, png, settled, until, upload } = require('./page');

const { canvasBox, screenshotPixels } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
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

/** A tool as the page names it (shown.ts toolName). */
const named = (tool) => (tool.package_title && tool.package_title !== tool.label ? `${tool.package_title} · ${tool.label}` : tool.label);

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-toolword-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };
  const folder = mkdtempSync(join(tmpdir(), 'web-toolword-'));
  writeFileSync(join(folder, 'take.png'), png(80));
  writeFileSync(join(folder, 'plain.png'), png(150));
  writeFileSync(join(folder, 'take.mp4'), MP4);

  // A picture with Qwen's Image and a video with H3's Shot, each with a take; a picture and words with no tool.
  const tools = [...((await api(`/v1/tools?project=${id}&q=qwen`)).body?.tools ?? []), ...((await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? [])];
  const image = tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image');
  const video = tools.find((tool) => tool.id === 'minimax-h3/generate') ?? tools.find((tool) => tool.package === 'minimax-h3' && tool.makes === 'video');
  if (!image || !video) throw new Error(`the Hub lists no Qwen Image or H3 Shot: ${JSON.stringify(tools.map((tool) => tool.id))}`);
  const post = async (path, body) => {
    const answer = await api(path, { method: 'POST', body: JSON.stringify(body) });
    if (answer.status >= 300) throw new Error(`${path}: ${JSON.stringify(answer)}`);
    return answer.body;
  };
  await post(`${at}/assets`, { tool: image.id, kind: 'image', name: 'still', exact: true, fields: { instructions: 'a lighthouse at dusk' }, at: { x: 100, y: 100 } });
  await post(`${at}/assets/still/versions`, { source: await upload(held?.token, join(folder, 'take.png'), id) });
  await post(`${at}/assets`, { tool: video.id, kind: 'video', name: 'shot', exact: true, fields: { summary: 'a lighthouse at dusk', beats: [{ description: 'Waves.', duration: 5 }] }, at: { x: 560, y: 100 } });
  await post(`${at}/assets/shot/versions`, { source: await upload(held?.token, join(folder, 'take.mp4'), id) });
  await post(`${at}/imports`, { source: await upload(held?.token, join(folder, 'plain.png'), id), name: 'plain', exact: true, at: { x: 1020, y: 100 } });
  await post(`${at}/assets`, { tool: '', name: 'words', exact: true, fields: { text: 'a quiet harbour' }, at: { x: 100, y: 560 } });
  for (const name of ['still', 'shot', 'plain', 'words']) await until('card on the canvas', () => canvasBox(page, name), Boolean);
  await fitCanvas(page);
  await settled(page);

  /** What a box says of its tool under its picture, and how large it's drawn against the box. */
  const labelColor=await page.evaluate(()=>{const probe=document.createElement('span');probe.style.color='var(--mute)';document.body.append(probe);const color=getComputedStyle(probe).color;probe.remove();const rgb=color.match(/[\d.]+/g).map(Number).slice(0,3);return color.startsWith('color(')?rgb.map(v=>v*255):rgb;});
  const said = async (name) => {
    await page.mouse.move(20,20);
    const card=await canvasBox(page,name), word=card?.rendered?.labels.tool;
    if (!word) return {text:null,card:card?.body.width};
    const read=screenshotPixels(await page.screenshot()),dpr=await page.evaluate(()=>devicePixelRatio);
    let ink=0;
    for(let y=word.y;y<word.y+word.height;y++) for(let x=word.x;x<word.x+word.width;x++) {
      if(x<0 || y<0 || x*dpr>=read.width || y*dpr>=read.height) continue;
      if(read(x*dpr,y*dpr).every((value,i)=>Math.abs(value-labelColor[i])<=35)) ink++;
    }
    return {text:word.fullText ?? word.text,size:word.fontSize,drawn:word.height,card:card.body.width,visible:word.width>20 && word.height>4 && ink>=3,ink};
  };
  const wanted = { still: named(image), shot: named(video) };
  seen.wanted = wanted;
  const near = {};
  for (const name of ['still', 'shot']) {
    near[name] = await until(`${name}'s tool`, () => said(name), (now) => now.text !== null).catch(() => said(name));
    seen[name] = near[name];
    check(`${name}_says_its_tool`, near[name].text === wanted[name] && near[name].visible, { got: near[name], wanted: wanted[name] });
    check(`${name}_tool_is_readable`, near[name].size >= 11, near[name]);
  }
  for (const name of ['plain', 'words']) {
    seen[name] = await said(name);
    check(`${name}_with_no_tool_says_nothing`, seen[name].text === null, seen[name]);
  }
  await shot('tools-on-boxes');

  // Zoomed out, short of far out (under 20% a box shows its picture and name alone: CanvasHost.tsx's band), it's still said and shrinks
  // with the box.
  const zoom = async () => Number((await page.getByRole('button', { name: /more zoom options$/ }).getAttribute('aria-label')).match(/(\d+)%/)?.[1]) / 100;
  await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
  await settled(page);
  const far = { ...(await said('still')), zoom: await zoom() };
  seen.zoomed_out = far;
  check('it_shrinks_with_the_box', far.zoom >= 0.2 && far.text !== null && far.card < near.still.card && Math.abs(far.drawn / far.card - near.still.drawn / near.still.card) < 0.25 * (near.still.drawn / near.still.card), { near: near.still, far });
  await shot('tools-zoomed-out');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
