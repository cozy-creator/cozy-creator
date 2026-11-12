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
const { projectFromPage, calcifer, signIn, STEP_MS, drop, lastLook, launch, lookAt, noStall, photo, png, until } = require('./page');

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
    .map(([w, h, alpha], i) => ({ name: `p${String(i + 1).padStart(2, '0')}`, type: 'image/png', ext: 'png', bytes: png(20 + i * 12, w, h, Boolean(alpha)), aspect: w / h })),
  ...[1, 3, 6, 8, 5].map((turn, i) => ({ name: `p${16 + i}`, type: 'image/jpeg', ext: 'jpg', bytes: turned(turn), aspect: turn >= 5 ? 64 / 96 : 96 / 64, turn })),
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
  const thumbs = [];
  page.on('request', (request) => { if (new URL(request.url()).pathname.includes('/thumbs/')) thumbs.push(request.url()); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path) => page.evaluate(async ({ path, token }) => {
    const answer = await fetch(path, { headers: token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {} });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-dropped-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };

  // Far enough out that all twenty are drawn: none culled, so none is drawn for the first time by a pan.
  const pane = page.locator('.react-flow__pane');
  await pane.waitFor();
  const scale = () => page.evaluate(() => Number(/scale\(([\d.]+)\)/.exec(document.querySelector('.react-flow__viewport')?.style.transform ?? '')?.[1] ?? 1));
  await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
  for (let i = 0; i < 8 && (await scale()) > 0.42; i++) {
    await page.keyboard.press('-');
    await page.waitForTimeout(250);
  }
  seen.zoom = await scale();
  check('zoomed_out_not_far', seen.zoom <= 0.42 && seen.zoom >= 0.2, seen.zoom);

  // Every frame, every box: where and how big it's drawn, its picture and its head, from the frame it's first drawn.
  await page.evaluate(() => {
    const boxes = (window.dropWatch = { first: {}, changes: [], bad: [], loads: [], frames: 0, stop: false });
    const sample = (node) => {
      const card = node.querySelector('.card');
      const media = card?.querySelector('.media');
      const imgs = [...(media?.querySelectorAll('.media-pixels img') ?? [])];
      return {
        name: card?.dataset.asset,
        pos: node.style.transform,
        w: card?.offsetWidth,
        h: card?.offsetHeight,
        mw: media?.offsetWidth,
        mh: media?.offsetHeight,
        srcs: imgs.map((img) => img.getAttribute('src')).join(' '),
        drawn: imgs.length > 0 && imgs.every((img) => img.complete && img.naturalWidth > 0),
        head: card?.querySelector('.head')?.textContent,
        visible: getComputedStyle(node).visibility === 'visible',
      };
    };
    // Where it's drawn, to a hundredth of a pixel: a box's origin is worked out in floats.
    const at = (pos) => (/translate\(([-\d.e]+)px, ?([-\d.e]+)px\)/.exec(pos ?? '') ?? []).slice(1).map(Number);
    const near = (a, b) => a.length === 2 && b.length === 2 && Math.abs(a[0] - b[0]) < 0.01 && Math.abs(a[1] - b[1]) < 0.01;
    const same = (a, b) => near(at(a.pos), at(b.pos)) && Math.abs(a.w - b.w) <= 0.5 && Math.abs(a.h - b.h) <= 0.5 && Math.abs(a.mw - b.mw) <= 0.5 && Math.abs(a.mh - b.mh) <= 0.5 && a.srcs === b.srcs && a.head === b.head;
    const tick = () => {
      if (boxes.stop) return;
      boxes.frames += 1;
      const when = Math.round(performance.now());
      for (const node of document.querySelectorAll('.react-flow__node')) {
        const id = node.dataset.id;
        const now = sample(node);
        if (!now.visible) continue;
        const first = boxes.first[id];
        if (!first) boxes.first[id] = { ...now, at: when };
        else if (!same(first, now) && boxes.changes.length < 60) boxes.changes.push({ id, at: when, first, now });
        const wrong = [node.classList.contains('glide') && 'glide', node.querySelector('.card.landing') && 'landing', node.querySelector('.prog') && 'progress', node.querySelector('.state-dot.queued, .state-dot.running') && 'state dot'].filter(Boolean);
        if (wrong.length && boxes.bad.length < 60) boxes.bad.push({ id, at: when, wrong });
      }
      requestAnimationFrame(tick);
    };
    document.addEventListener('load', (event) => {
      const node = event.target instanceof HTMLImageElement && event.target.closest('.react-flow__node');
      if (node) boxes.loads.push({ id: node.dataset.id, src: event.target.getAttribute('src') });
    }, true);
    requestAnimationFrame(tick);
  });

  const files = PICTURES.map(({ name, type, ext, bytes }) => ({ name: `${name}.${ext}`, type, bytes }));
  const dropped = Date.now();
  await drop(page, files, 700, 160);
  const assets = async () => (await api(at)).body?.assets ?? [];
  const landed = await until('all twenty landed', () => page.evaluate(() => document.querySelectorAll('.react-flow__node .head [aria-label="Delete"]').length), (n) => n === PICTURES.length, 5 * STEP_MS);
  seen.landed_ms = Date.now() - dropped;
  check('twenty_boxes', landed === PICTURES.length, landed);
  await shot('placed');
  const placed = await page.evaluate(() => Object.fromEntries([...document.querySelectorAll('.react-flow__node')].map((node) => [node.dataset.id, { pos: node.style.transform, w: node.querySelector('.card')?.offsetWidth, h: node.querySelector('.card')?.offsetHeight }])));
  // Calcifer measures each file after its import: that read comes back to the page too, and changes nothing on it.
  // As a box reads it: its first version's file, else its own.
  const dims = (row) => {
    const main = row.prefixes?.find((version) => version.number === 1)?.main ?? row.prefixes?.[0]?.main;
    return main?.width && main.height ? main : row;
  };
  const measured = await until('all twenty measured', assets, (rows) => rows.length === PICTURES.length && rows.every((row) => dims(row).width && dims(row).height), 3 * STEP_MS);
  await page.waitForTimeout(10_000);
  const later = await page.evaluate(() => Object.fromEntries([...document.querySelectorAll('.react-flow__node')].map((node) => [node.dataset.id, { pos: node.style.transform, w: node.querySelector('.card')?.offsetWidth, h: node.querySelector('.card')?.offsetHeight }])));
  const watched = await page.evaluate(() => {
    window.dropWatch.stop = true;
    return window.dropWatch;
  });
  await shot('ten-seconds-later');

  const ids = Object.keys(watched.first);
  seen.frames = watched.frames;
  check('each_drawn_once_each_frame', ids.length === PICTURES.length, ids.length);
  check('each_box_first_drawn_with_its_picture', ids.every((id) => watched.first[id].drawn), ids.filter((id) => !watched.first[id].drawn).map((id) => watched.first[id]));
  check('each_box_first_drawn_with_its_own_file', ids.every((id) => watched.first[id].srcs.startsWith('blob:') && !watched.first[id].srcs.includes(' ')), ids.map((id) => watched.first[id].srcs));
  check('nothing_moves_resizes_or_swaps_after_it_is_drawn', watched.changes.length === 0, watched.changes.slice(0, 6));
  check('no_progress_dot_landing_or_glide', watched.bad.length === 0, watched.bad.slice(0, 6));
  const xy = (pos) => (/translate\(([-\d.e]+)px, ?([-\d.e]+)px\)/.exec(pos ?? '') ?? []).slice(1).map(Number);
  const kept = Object.keys(placed).length === PICTURES.length && Object.entries(placed).every(([id, box]) => {
    const [a, b] = [xy(box.pos), xy(later[id]?.pos)];
    return later[id] && a.length === 2 && b.length === 2 && Math.abs(a[0] - b[0]) < 0.01 && Math.abs(a[1] - b[1]) < 0.01 && box.w === later[id].w && box.h === later[id].h;
  });
  check('placed_and_ten_seconds_later_the_same', kept, { placed, later });
  const loads = Object.groupBy(watched.loads, (load) => load.id);
  check('each_picture_loaded_once', ids.every((id) => (loads[id] ?? []).length <= 1 && (loads[id] ?? []).every((load) => load.src === watched.first[id].srcs)), loads);
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
  await page.waitForSelector('.react-flow__node .card');
  const reloaded = await until('every thumbnail up after a reload', () => page.evaluate(() => [...document.querySelectorAll('.react-flow__node')].map((node) => {
    const card = node.querySelector('.card');
    const media = card?.querySelector('.media');
    const img = media?.querySelector('.buffered-image[data-displayed-sha] img');
    return { name: card?.dataset.asset, w: card?.offsetWidth, h: card?.offsetHeight, media: media && media.offsetWidth / media.offsetHeight, picture: img && img.naturalWidth / img.naturalHeight };
  })), (all) => all.length === PICTURES.length && all.every((box) => box.picture), 3 * STEP_MS);
  await shot('reloaded');
  const before = Object.fromEntries(ids.map((id) => [watched.first[id].name, watched.first[id]]));
  check('a_reload_keeps_each_shape', reloaded.every((box) => before[box.name] && Math.abs(box.w - before[box.name].w) <= 1 && Math.abs(box.h - before[box.name].h) <= 1), reloaded.map((box) => ({ ...box, before: before[box.name] && { w: before[box.name].w, h: before[box.name].h } })));
  check('thumbnails_upright', reloaded.every((box) => Math.abs(box.picture / box.media - 1) < 0.03), reloaded);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
