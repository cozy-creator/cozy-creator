// Twenty pictures dropped at once (cl-918, Paul 11-12: "hide this import phase ... Do not move nodes once they are placed"), in a real browser on a real
// Calcifer. Mixed sizes: PNGs from 1:2.5 to 3:1, two with alpha, and a camera's JPEG stored as it was shot and turned by its EXIF (3, 5, 6, 8). Watched every
// frame from the drop: each box is first drawn at its final place and shape with its picture already decoded, and from then on, through its upload, its
// import and Calcifer's measurement of it, and ten seconds after, no box moves, resizes, swaps or reloads its picture, changes its head, or shows a
// progress bar, a pulsing dot, a landing or a glide; no thumbnail is asked for. Calcifer measures a turned JPEG as it's drawn; after a reload each box
// is the same shape, and its thumbnail is drawn upright.
//   node dropped.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1 if
// anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { createHash } = require('node:crypto');
const { projectFolder, projectFromPage, calcifer, signIn, STEP_MS, drop, lastLook, launch, lookAt, noStall, photo, png, settled, until, canvasBox, canvasView, canvasNames, selectedBoxes, hoverBox, screenshotPixels } = require('./page');

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

/** A camera's JPEG with an EXIF Orientation of `turn`, its first segment, as a phone writes it: photo()'s 96×64 stored. */
function turned(turn) {
  const tiff = Buffer.from([0x4d, 0x4d, 0x00, 0x2a, 0x00, 0x00, 0x00, 0x08, 0x00, 0x01, 0x01, 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, turn, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
  const app1 = Buffer.concat([Buffer.from([0xff, 0xe1, 0x00, 2 + 6 + tiff.length]), Buffer.from('Exif\0\0', 'binary'), tiff]);
  const bytes = photo();
  return Buffer.concat([bytes.subarray(0, 2), app1, bytes.subarray(2)]);
}

// Each picture and the shape it's drawn at.
const PICTURES = [
  ...[[320, 200], [200, 320], [640, 480], [300, 300], [900, 300], [300, 750], [800, 450], [256, 512], [1024, 576], [480, 640], [500, 200], [210, 297], [333, 222, true], [128, 128, true], [1200, 900]]
    .map(([w, h, alpha], i) => ({ name: `p${String(i + 1).padStart(2, '0')}`, type: 'image/png', ext: 'png', bytes: png(20 + i * 12, w, h, Boolean(alpha)), width: w, height: h, aspect: w / h })),
  ...[1, 3, 6, 8, 5].map((turn, i) => ({ name: `p${16 + i}`, type: 'image/jpeg', ext: 'jpg', bytes: turned(turn), width: turn >= 5 ? 64 : 96, height: turn >= 5 ? 96 : 64, aspect: turn >= 5 ? 64 / 96 : 96 / 64, turn })),
];

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1920, height: 1200 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const sources=PICTURES.map(picture=>({name:picture.name,sha256:createHash('sha256').update(picture.bytes).digest('hex'),width:picture.width,height:picture.height,decodedBytes:picture.width*picture.height*4}));
  const thumbs=[],sourceReads=[];
  page.on('request',request=>{
    if(request.method()!=='GET')return;
    const url=new URL(request.url());
    if(url.pathname.includes('/thumbs/'))thumbs.push(url.pathname);
    const target=decodeURIComponent(url.pathname)+' '+(url.searchParams.get('path') ?? '');
    const source=sources.find(source=>target.includes(source.sha256) || target.includes(source.sha256.slice(0,12)));
    if(source)sourceReads.push({name:source.name,kind:url.pathname.includes('/thumbs/')?'thumbnail':'file'});
  });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path) => page.evaluate(async ({ path, token }) => {
    const answer = await fetch(path, { headers: token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {} });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-dropped-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };

  // Far enough out that all twenty are drawn: none culled, so none is drawn for the first time by a pan.
  const pane = page.locator('.canvas-pixels');
  await pane.waitFor();
  const scale = async () => (await canvasView(page)).zoom;
  await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
  for (let i = 0; i < 8 && (await scale()) > 0.42; i++) {
    await page.keyboard.press('-');
    // Each step's zoom done before the next is judged: a slow runner's half-done step would read as one more press needed.
    await settled(page);
  }
  seen.zoom = await scale();
  check('zoomed_out_not_far', seen.zoom <= 0.42 && seen.zoom >= 0.2, seen.zoom);

  // Display-object diagnostics catch transient placement/resource changes; independent screenshots below establish actual pixels.
  await page.evaluate(() => {
    const boxes = (window.dropWatch = { first: {}, changes: [], bad: [], loads: [], ready: {}, frames: 0, stop: false });
    const tick = () => {
      if (boxes.stop) return;
      boxes.frames++;
      const inspection = document.querySelector('.canvas-root')?.canvasInspection;
      const zoom = inspection?.camera().zoom;
      for (const name of inspection?.names() ?? []) {
        const box = inspection.card(name), render = box?.rendered;
        if (!render) continue;
        const now = { name, pos: JSON.stringify([box.bounds.x, box.bounds.y]), w: box.bounds.width / zoom, h: box.bounds.height / zoom,
          mw: render.body.width / zoom, mh: render.body.height / zoom, srcs: render.media.local ? 'local' : render.media.resolution,
          drawn: render.media.ready, head: render.labels.title?.text, visible: true };
        const first = boxes.first[name];
        if (now.drawn && !boxes.ready[name]) boxes.loads.push({ id:name,src:now.srcs,at:Math.round(performance.now()) });
        boxes.ready[name]=now.drawn;
        if (!first) { boxes.first[name] = { ...now, at: Math.round(performance.now()) }; }
        else if (['pos', 'srcs', 'head'].some(key => first[key] !== now[key]) || ['w','h','mw','mh'].some(key => Math.abs(first[key] - now[key]) > 0.5)) {
          if (boxes.changes.length < 60) boxes.changes.push({ id: name, first, now });
        }
        const wrong = [render.progress && 'progress', ['queued', 'running'].includes(render.state) && 'state dot', render.landing?.active && 'landing animation'].filter(Boolean);
        if (wrong.length && boxes.bad.length < 60) boxes.bad.push({ id: name, wrong });
      }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  const poses = async () => Object.fromEntries(await Promise.all(PICTURES.map(async ({ name }) => {
    const box = await canvasBox(page, name), zoom = (await canvasView(page)).zoom;
    return [name, box && { pos: JSON.stringify([box.bounds.x, box.bounds.y]), w: box.bounds.width / zoom, h: box.bounds.height / zoom }];
  })));
  const pixelsOf = async () => {
    await page.mouse.move(4, 4);
    const read = screenshotPixels(await page.screenshot());
    const result = {};
    for (const { name } of PICTURES) {
      const body = (await canvasBox(page, name))?.rendered?.body;
      if (!body) return null;
      result[name] = [[0.25, 0.25], [0.75, 0.25], [0.25, 0.75], [0.75, 0.75]].map(([x, y]) => read(body.x + body.width * x, body.y + body.height * y));
    }
    return result;
  };
  const pngPixelsMatch = (pixels) => pixels && PICTURES.slice(0, 15).every((picture, i) => pixels[picture.name].every((color, quadrant) => {
    if ([12, 13].includes(i) && quadrant % 2 === 0) return true;
    const expected = [20 + i * 12, quadrant % 2 ? 191 : 64, quadrant >= 2 ? 191 : 64];
    return color.every((channel, c) => Math.abs(channel - expected[c]) <= 20);
  }));

  const resources=()=>page.evaluate(()=>document.querySelector('.canvas-root')?.canvasInspection?.resources());
  const beforeDecode=await until('empty scene decode counter',resources,stats=>Number.isFinite(stats?.decodedTotalBytes));
  const files = PICTURES.map(({ name, type, ext, bytes }) => ({ name: `${name}.${ext}`, type, bytes }));
  const dropped = Date.now();
  await drop(page, files, 700, 160);
  const assets = async () => (await api(at)).body?.assets ?? [];
  const landed = await until('all twenty landed', async () => (await assets()).length, (n) => n === PICTURES.length, 5 * STEP_MS);
  seen.landed_ms = Date.now() - dropped;
  check('twenty_boxes', landed === PICTURES.length, landed);
  await shot('placed');
  const placed = await poses();
  const firstPixels = await until('all dropped PNGs have their actual colors', pixelsOf, pngPixelsMatch);
  check('dropped_image_pixels', pngPixelsMatch(firstPixels), firstPixels);
  // Calcifer measures each file after its import: that read comes back to the page too, and changes nothing on it.
  // As a box reads it: its first version's file, else its own.
  const dims = (row) => {
    const main = row.prefixes?.find((version) => version.number === 1)?.main ?? row.prefixes?.[0]?.main;
    return main?.width && main.height ? main : row;
  };
  const measured = await until('all twenty measured', assets, (rows) => rows.length === PICTURES.length && rows.every((row) => dims(row).width && dims(row).height), 3 * STEP_MS);
  await page.waitForTimeout(10_000);
  const later = await poses();
  const laterPixels = await pixelsOf();
  check('displayed_pixels_do_not_change_after_import', JSON.stringify(firstPixels) === JSON.stringify(laterPixels), { firstPixels, laterPixels });
  const watched = await page.evaluate(() => {
    window.dropWatch.stop = true;
    return window.dropWatch;
  });
  await shot('ten-seconds-later');

  const ids = Object.keys(watched.first);
  seen.frames = watched.frames;
  check('each_drawn_once_each_frame', ids.length === PICTURES.length, ids.length);
  check('each_box_first_drawn_with_its_picture', ids.every((id) => watched.first[id].drawn), ids.filter((id) => !watched.first[id].drawn).map((id) => watched.first[id]));
  check('each_box_first_drawn_with_its_own_file', ids.every((id) => watched.first[id].srcs === 'local'), ids.map((id) => watched.first[id].srcs));
  check('nothing_moves_resizes_or_swaps_after_it_is_drawn', watched.changes.length === 0, watched.changes.slice(0, 6));
  check('no_progress_dot_landing_or_glide', watched.bad.length === 0 && !watched.changes.some(change=>change.first.pos!==change.now.pos), watched.bad.slice(0, 6));
  const xy = (pos) => JSON.parse(pos ?? '[]');
  const kept = Object.keys(placed).length === PICTURES.length && Object.entries(placed).every(([id, box]) => {
    const [a, b] = [xy(box.pos), xy(later[id]?.pos)];
    return later[id] && a.length === 2 && b.length === 2 && Math.abs(a[0] - b[0]) < 0.01 && Math.abs(a[1] - b[1]) < 0.01 && box.w === later[id].w && box.h === later[id].h;
  });
  check('placed_and_ten_seconds_later_the_same', kept, { placed, later });
  const loads = Object.groupBy(watched.loads, (load) => load.id);
  const afterDecode=await until('scene decodes settled',resources,stats=>stats?.loading===0 && stats?.queued===0 && stats?.loadedCards===PICTURES.length);
  const distinct=new Map(sources.map(source=>[source.sha256,source]));
  const expectedDecodedBytes=[...distinct.values()].reduce((sum,source)=>sum+source.decodedBytes,0);
  const decodedBytes=afterDecode.decodedTotalBytes-beforeDecode.decodedTotalBytes;
  seen.source_decode_evidence={sources:[...distinct.values()],expectedDecodedBytes,decodedBytes,before:beforeDecode,after:afterDecode,readyTransitions:loads,sourceReads};
  // Aggregate decoded-byte work is independent of first-card observations. Alongside one ready transition per distinct source,
  // stable local identity and zero file/thumbnail GETs, it detects repeated raw decodes in this fixed all-visible workload.
  // It does not claim to count driver uploads or the browser's separate import-decoding step.
  check('each_picture_loaded_once',distinct.size===PICTURES.length && decodedBytes===expectedDecodedBytes && afterDecode.failed===0 && ids.every(id=>(loads[id]??[]).length===1 && loads[id][0].src===watched.first[id].srcs),seen.source_decode_evidence);
  check('fresh_sources_are_not_fetched_again',sourceReads.length===0 && afterDecode.fetchedBytes===beforeDecode.fetchedBytes,sourceReads);
  check('no_thumbnail_asked', thumbs.length === 0, thumbs.slice(0, 5));
  // Each at its own shape: its picture's box is as the file is drawn, a turned JPEG standing up.
  const byName = Object.fromEntries(ids.map((id) => [watched.first[id].name, watched.first[id]]));
  const shapes = PICTURES.map(({ name, aspect }) => ({ name, aspect, drawn: byName[name] && byName[name].mw / byName[name].mh }));
  check('each_box_its_pictures_shape', shapes.every(({ aspect, drawn }) => drawn && Math.abs(drawn / aspect - 1) < 0.02), shapes);
  const rows = Object.fromEntries(measured.map((row) => [row.name, dims(row).width / dims(row).height]));
  const turnedRows = PICTURES.filter(({ turn }) => turn).map(({ name, aspect }) => ({ name, aspect, measured: rows[name] }));
  check('calcifer_measures_a_turned_jpeg_as_drawn', turnedRows.every(({ aspect, measured: ratio }) => Math.abs(ratio / aspect - 1) < 0.02), turnedRows);
  check('no_box_lies_on_another', (() => {
    const boxes = ids.map((id) => {
      const [x, y] = xy(watched.first[id].pos);
      return { x, y, w: watched.first[id].w, h: watched.first[id].h };
    });
    return boxes.every((a, i) => boxes.every((b, j) => i === j || a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y));
  })());

  // A reload: the same shapes, from what Calcifer measured, and each picture's thumbnail upright in it.
  await page.reload();
  await until('all twenty logical cards after reload', () => canvasNames(page), names => names?.length === PICTURES.length);
  const reloaded = await until('every thumbnail decoded after a reload', async () => Promise.all(PICTURES.map(async ({ name }) => {
    const box = await canvasBox(page, name), zoom = (await canvasView(page)).zoom, render = box?.rendered;
    return { name, w: box?.bounds.width / zoom, h: box?.bounds.height / zoom,
      media: render && render.body.width / render.body.height, picture: render?.media.ready && render.media.width / render.media.height };
  })), all => all.length === PICTURES.length && all.every(box => box.picture), 3 * STEP_MS);
  const reloadPixels = await until('reloaded PNGs retain their displayed colors', pixelsOf, pngPixelsMatch);
  check('reloaded_image_pixels', pngPixelsMatch(reloadPixels), reloadPixels);
  const upright = PICTURES.filter(picture => picture.turn).every(({ name }) => reloadPixels[name].every((color, i) => color.every((channel, c) => Math.abs(channel - firstPixels[name][i][c]) <= 25)));
  check('turned_JPEG_pixels_match_before_and_after_reload', upright, { firstPixels, reloadPixels });
  await shot('reloaded');
  const before = Object.fromEntries(ids.map((id) => [watched.first[id].name, watched.first[id]]));
  check('a_reload_keeps_each_shape', reloaded.every((box) => before[box.name] && Math.abs(box.w - before[box.name].w) <= 1 && Math.abs(box.h - before[box.name].h) <= 1), reloaded.map((box) => ({ ...box, before: before[box.name] && { w: before[box.name].w, h: before[box.name].h } })));
  check('thumbnails_upright', reloaded.every((box) => Math.abs(box.picture / box.media - 1) < 0.03), reloaded);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
