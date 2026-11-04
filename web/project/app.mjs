// A project's page: one more client of Calcifer's HTTP API, holding display and YAML editing and no project logic.
import {connect, parseLink, play} from "/page/cozy-webrtc.js";
import {add, nameKey, read, write} from "/page/lines.mjs";

const id = location.pathname.split("/")[2];
const base = `/v1/projects/${id}`;
const WHOLE = "project.yaml";
const $ = selector => document.querySelector(selector);

function h(spec, props = {}, ...kids) {
  const [tag, ...classes] = spec.split(".");
  const element = Object.assign(document.createElement(tag), props);
  if (classes.length) element.className = classes.join(" ");
  element.append(...kids.flat().filter(kid => kid != null && kid !== false));
  return element;
}

let project = null, detail = null;  // GET /v1/projects/{id}, and /assets/{selected}
let selected = decodeURIComponent(location.hash.slice(1)) || null;
let chosen = null;                  // The version shown: its number, {run} for one being made, null for the asset's state.
let editing = null;                 // WHOLE while the whole file is open, else null: the selected asset's lines.
let file = "", etag = "";           // project.yaml as last read.
const drafts = new Map();           // What's typed and not saved, by asset: {base, text}.
const runs = new Map();             // Runs being made, as last read.
let conflict = null;

// ---- Asking Calcifer ----

function ended(words) {
  $("#banner").textContent = words;
  $("#banner").hidden = false;
}

async function ask(path, options) {
  let answer;
  try {
    answer = await fetch(path, options);
  } catch {
    ended("Calcifer isn't answering. Start it again with cozy project open.");
    throw new Error("Calcifer isn't answering");
  }
  if (answer.status === 401) ended("This page's session ended when Calcifer restarted. Run cozy project open again.");
  if (answer.status >= 400) {
    const {error} = await answer.json().catch(() => ({}));
    const refused = new Error([error?.message ?? `Calcifer answered ${answer.status}`, error?.remedy].filter(Boolean).join(": "));
    refused.code = error?.code;
    throw refused;
  }
  const text = answer.status === 204 ? "" : await answer.text();
  return text ? JSON.parse(text) : null;
}

const post = (path, body) => ask(path, {method: "POST", headers: {"Content-Type": "application/json", "Idempotency-Key": crypto.randomUUID()}, body: JSON.stringify(body)});

// ---- The asset list and the bar across the top ----

const counted = (n, words) => n ? `${n} ${words}` : null;
const bytes = n => n > 1e9 ? `${(n / 1e9).toFixed(1)} GB` : `${Math.round(n / 1e6)} MB`;
const cutOf = asset => project.cuts?.find(cut => cut.cut === `${asset.name.replace(/[/\\]/g, "_")}.mp4`);

function renderTop() {
  const name = project.path.split(/[/\\]/).pop();
  $("#name").textContent = name;
  document.title = `${name}: cozy`;
  const of = standing => project.assets.filter(asset => asset.standing === standing).length;
  const orphans = project.orphans ?? [];
  $("#summary").textContent = [
    counted(of("making"), "making"), counted(of("out_of_date"), "out of date"), counted(of("missing"), "missing"),
    project.absent ? `${project.absent} files not here: cozy project pull` : null,
    orphans.length ? `${orphans.length} orphaned, ${bytes(orphans.reduce((sum, orphan) => sum + orphan.bytes, 0))}: cozy project reclaim` : null,
  ].filter(Boolean).join(" · ") || "All current";
  $("#cuts").replaceChildren(...project.assets.filter(cutOf).map(asset =>
    h(`button${asset.name === selected ? ".now" : ""}`, {onclick: () => choose(asset.name), title: `Play ${cutOf(asset).cut}`}, asset.name)));
}

function renderList() {
  $("#assets").replaceChildren(...project.assets.map(asset => {
    const state = asset.standing.replaceAll("_", " ");
    const classes = [!editing && asset.name === selected && "now", drafts.has(asset.name) && "unsaved"].filter(Boolean).join(".");
    return h(`button.row${classes ? `.${classes}` : ""}`, {onclick: () => choose(asset.name), title: asset.status},
      h(`span.dot.${asset.standing}`, {title: state, ariaLabel: state}),
      h("span.name", {}, asset.name),
      asset.pinned && asset.with && h("span.pin", {title: "Pinned: a hash in project.yaml"}, "PIN"),
      h("span.kind", {}, asset.kind));
  }));
  $("#whole").classList.toggle("now", editing === WHOLE);
  $("#whole").classList.toggle("unsaved", drafts.has(WHOLE));
}

// ---- The machines Render can use ----

async function refreshMachines() {
  const {machines} = await ask("/v1/machines");
  const usable = machines.filter(m => m.recorded !== false && !m.offline && ["ready", "booting", "stopped"].includes(m.state));
  const select = $("#machine"), kept = select.value || localStorage.getItem("cozy-machine");
  const words = m => [m.name, m.rental ? `${m.rental.gpus}× ${m.rental.accelerator}` : m.status?.gpus?.[0]?.name, m.state === "booting" && "booting"].filter(Boolean).join(" · ");
  select.replaceChildren(...usable.map(m => h("option", {value: m.name}, words(m))));
  const rental = usable.find(m => m.rental);
  select.value = usable.some(m => m.name === kept) ? kept : (rental ?? usable[0])?.name ?? "";
  renderActions();
}

// ---- What the stage shows ----

const main = files => files?.video ?? files?.image ?? files?.audio ?? Object.values(files ?? {})[0];
const fileUrl = found => `${base}/files/${found.path.split("/").pop()}`;
const shownVersion = () => detail?.versions.find(version => version.number === chosen);
const sameFile = (a, b) => a && b && a.sha256 === b.sha256;

let stageKey = null, player = null, muted = true;

function media(path, url) {
  if (/\.(mp4|webm|mov|m4v)$/i.test(path)) {
    const video = h("video", {src: url, controls: true, autoplay: true, loop: true, muted, playsInline: true});
    video.addEventListener("volumechange", () => { muted = video.muted; });
    video.addEventListener("timeupdate", follow);
    return video;
  }
  if (/\.(png|jpe?g|webp|gif)$/i.test(path)) return h("img", {src: url, alt: selected});
  if (/\.(wav|mp3|flac|m4a|ogg|aac)$/i.test(path)) return h("audio", {src: url, controls: true});
  return h("p.empty", {}, path.split("/").pop(), " is no picture, video or sound.");
}

// A run's output, straight from its machine over WebRTC (`cozy run play`'s route): a video as it grows, a picture whole.
async function fromMachine(run, output, picture) {
  const given = await ask(`/v1/runs/${run.id}/play?output=${encodeURIComponent(output)}`);
  const fragment = new URLSearchParams({v: 1, a: given.address, f: given.fingerprint, c: given.token, r: run.id, o: output});
  const link = parseLink(`#${fragment}`);
  $("#note").textContent = `From ${run.machine}, over WebRTC: this computer doesn't hold it yet.`;
  if (!picture) {
    const video = media("a.mp4", "");
    video.removeAttribute("src");
    player = play(video, link);
    player.addEventListener("status", ({detail: {state, detail: words}}) => { if (state === "error") $("#note").textContent = words; });
    return video;
  }
  const session = await connect(link, {cap: link.cap});
  const chunks = [];
  const bytes = await new Promise((done, fail) => session.follow({run: run.id, output}, {
    data: (_, chunk) => { chunks.push(chunk.slice()); session.release(chunk.length); },
    end: () => done(new Blob(chunks)),
    error: fail,
  }));
  session.close();
  return h("img", {src: URL.createObjectURL(bytes), alt: selected});
}

// A file of a version: from here if it is here, else from the machine whose run made it, if that run did (a lock from another
// computer names its runs by that computer's numbers, so the hash has to match).
async function fileMedia(found, version) {
  const url = fileUrl(found);
  if ((await fetch(url, {method: "HEAD"})).ok) return media(found.path, url);
  for (const number of [...(version?.runs ?? [])].reverse()) {
    const run = await ask(`/v1/runs/${number}`).catch(() => null);
    const item = run?.output?.find(item => item.sha256 === found.sha256);
    if (item) return fromMachine(run, item.name, /\.(png|jpe?g|webp|gif)$/i.test(found.path));
  }
  return h("p.empty", {}, "This file is on no machine and not on this computer: ", h("code", {}, "cozy project pull"));
}

function wanted() {
  const asset = detail.asset, cut = cutOf(asset);
  if (chosen?.run) return {key: `run ${chosen.run}`, make: async () => {
    const run = await ask(`/v1/runs/${chosen.run}`);
    if (asset.noun === "take" && run.machine) return fromMachine(run, "video");
    return h("p.empty", {}, asset.noun === "take" ? "Waiting for a machine." : "Being made: it shows here when it lands.");
  }};
  const version = shownVersion();
  if (!version && cut?.cut_at_ms) {
    const url = `${base}/cuts/${cut.cut}?t=${cut.cut_at_ms}`;
    return {key: url, make: async () => media(cut.cut, url)};
  }
  const found = main(version?.files ?? asset.files);
  if (found) return {key: `${found.sha256} ${selected}`, make: () => fileMedia(found, version)};
  return {key: `nothing ${selected}`, make: async () => h("p.empty", {}, asset.status)};
}

async function renderStage() {
  const want = wanted();
  if (stageKey === want.key) return;
  stageKey = want.key;
  player?.close();
  player = null;
  $("#stage").replaceChildren(h("p.empty", {}, "Loading…"));
  $("#note").textContent = "";
  let shown;
  try {
    shown = await want.make();
  } catch (error) {
    shown = h("p.empty.error", {}, error.message);
  }
  if (stageKey === want.key) $("#stage").replaceChildren(shown);
  renderNote();
  renderActions();
}

function renderNote() {
  if (!detail || player) return;
  const asset = detail.asset, version = shownVersion(), cut = cutOf(asset);
  const facts = cut?.frames ? [`${cut.frames} frames at ${cut.fps} fps`, ...(cut.retimed ?? []).map(take => `retimed ${take}`), ...(cut.skips ?? []).map(skip => `skips ${skip}`)] : [];
  const about = version ? [`${asset.noun} ${version.number}`, version.seed != null && `seed ${version.seed}`, version.kept && `keeps take ${version.kept.take} to ${version.kept.until_s.toFixed(2)} s`, ...(version.warnings ?? [])] : [];
  $("#note").textContent = [asset.status, ...facts, ...about].filter(Boolean).join(" · ");
  $("#note").classList.toggle("stale", asset.standing === "out_of_date");
}

// ---- The strip of versions ----

const thumbs = new Map();
let thumbing = Promise.resolve();

async function draw(found) {
  const video = /\.(mp4|webm|mov|m4v)$/i.test(found.path);
  const source = document.createElement(video ? "video" : "img");
  await new Promise((done, fail) => {
    source.onerror = fail;
    if (video) {
      source.muted = true;
      source.preload = "auto";
      source.onloadeddata = () => { source.currentTime = Math.min(1, source.duration / 2); };
      source.onseeked = done;
    } else {
      source.onload = done;
    }
    source.src = fileUrl(found);
  });
  const canvas = h("canvas", {width: 192, height: 108});
  canvas.getContext("2d").drawImage(source, 0, 0, canvas.width, canvas.height);
  if (video) { source.removeAttribute("src"); source.load(); }
  return new Promise(done => canvas.toBlob(blob => done(URL.createObjectURL(blob))));
}

// One at a time: six videos loading at once would take all six of a browser's connections to Calcifer.
function thumb(found) {
  if (!thumbs.has(found.sha256)) thumbs.set(found.sha256, thumbing = thumbing.then(() => draw(found)).catch(() => null));
  return thumbs.get(found.sha256);
}

function versionTile(asset, version) {
  const image = h("img", {alt: ""});
  const found = main(version.files);
  if (found) thumb(found).then(url => { if (url) image.src = url; else image.hidden = true; });
  const now = chosen === version.number ? ".now" : "";
  return h(`button.tile${version.approved ? ".pinned" : ""}${now}`, {onclick: () => { chosen = version.number; renderMiddle(); }, title: `${asset.noun} ${version.number}${version.seed != null ? `, seed ${version.seed}` : ""}`},
    image, h("span.number", {}, version.number), version.approved && h("span.pin", {}, "PIN"));
}

function makingTile(number) {
  const now = chosen?.run === number ? ".now" : "";
  return h(`button.tile.making${now}`, {id: `run-${number}`, onclick: () => { chosen = {run: number}; renderMiddle(); }, title: `Run ${number}: play what it has so far`},
    h("span.words", {}, `Run ${number}`), h("span.bar"));
}

function renderProgress() {
  for (const number of detail?.asset.making ?? []) {
    const tile = document.getElementById(`run-${number}`), run = runs.get(number);
    if (!tile || !run) continue;
    const done = run.progress?.done_fraction;
    const words = run.stalled ? "No progress: stalled" : run.queue_position ? `Queued, ${run.queue_position} next` : [done != null && `${Math.round(done * 100)}%`, run.progress?.stage].filter(Boolean).join(" ") || run.status;
    tile.querySelector(".words").textContent = words;
    tile.querySelector(".bar").style.width = `${Math.round((done ?? 0) * 100)}%`;
  }
}

let stripKey = null;

function renderStrip() {
  const asset = detail.asset;
  const key = JSON.stringify([selected, asset.making, chosen, detail.versions.map(version => [version.number, version.approved])]);
  if (key === stripKey) return;
  stripKey = key;
  const tiles = [...detail.versions.map(version => versionTile(asset, version)), ...(asset.making ?? []).map(makingTile)];
  $("#strip").replaceChildren(...(tiles.length ? tiles : [h("span.none", {}, asset.with ? `No ${asset.noun} yet.` : "A constant, one file: nothing is made of it.")]));
  renderProgress();
}

// ---- A cut's pieces, along its length ----

let timelineKey = null, clips = [];

async function renderTimeline() {
  const cut = cutOf(detail.asset);
  const key = cut?.cut_at_ms ? `${cut.cut} ${cut.cut_at_ms}` : null;
  if (key === timelineKey) return;
  timelineKey = key;
  clips = [];
  $("#timeline").hidden = true;
  if (!key) return;
  const otio = await ask(`${base}/cuts/${cut.cut.replace(/\.mp4$/, ".otio")}?t=${cut.cut_at_ms}`).catch(() => null);
  let at = 0;
  for (const clip of otio?.tracks?.children?.[0]?.children ?? []) {
    const length = clip.source_range ? clip.source_range.duration.value / clip.source_range.duration.rate : 0;
    clips.push({name: clip.name, from: at, to: at += length});
  }
  $("#timeline").replaceChildren(...clips.map(clip => {
    clip.button = h("button", {onclick: () => { const video = $("#stage video"); if (video) video.currentTime = clip.from; }, ondblclick: () => choose(clip.name), title: `${clip.name}: ${(clip.to - clip.from).toFixed(1)} s. Click to go there, double-click to open it.`}, clip.name);
    clip.button.style.flex = `${clip.to - clip.from} 1 0`;
    return clip.button;
  }));
  $("#timeline").hidden = !clips.length;
}

function follow(event) {
  for (const clip of clips) clip.button.classList.toggle("now", event.target.currentTime >= clip.from && event.target.currentTime < clip.to);
}

function renderMiddle() {
  if (!detail) return;
  renderStage();
  renderStrip();
  renderTimeline();
  renderNote();
  renderActions();
}

// ---- The text ----

const editedName = () => editing ?? selected;
const onFile = name => name === WHOLE ? file : read(file, name)?.text;

function renderText() {
  const name = editedName(), area = $("#yaml"), held = onFile(name), draft = drafts.get(name);
  const asset = detail?.asset;
  $("#title").textContent = name ?? "";
  $("#about").textContent = editing ? "The whole file." : asset ? [asset.kind, asset.with].filter(Boolean).join(" · ") : "";
  area.disabled = held == null;
  const value = draft?.text ?? held ?? "";
  if (area.dataset.name !== name || area.value !== value) {
    area.value = value;
    area.dataset.name = name ?? "";
    area.scrollTop = 0;
  }
  area.placeholder = held == null && asset ? `${asset.name} has no lines of its own: a beat writes it.` : "";
  renderSaving();
  const known = new Map(project.assets.map(a => [nameKey(a.name), a.name]));
  const mentioned = new Set([...value.matchAll(/<([^<>\n]+)>/g)].map(match => known.get(nameKey(match[1]))).filter(Boolean));
  $("#reads").replaceChildren(...[...mentioned].map(read => h("button", {onclick: () => choose(read), title: `Open ${read}`}, read)));
}

function renderSaving() {
  const name = editedName(), dirty = drafts.has(name), saved = $("#saved");
  $("#save").disabled = !dirty;
  saved.classList.remove("error");
  if (conflict?.name === name) {
    saved.classList.add("error");
    saved.replaceChildren("The file changed under this.",
      h("button", {onclick: () => { drafts.get(name).base = conflict.now; conflict = null; save(name); }}, "Keep mine"),
      h("button", {onclick: () => { drafts.delete(name); conflict = null; renderText(); renderList(); }}, "Take the file's"));
  } else if (!saved.dataset.error || !dirty) {
    delete saved.dataset.error;
    saved.textContent = dirty ? "Not saved." : "";
  } else {
    saved.classList.add("error");
    saved.textContent = saved.dataset.error;
  }
}

$("#yaml").addEventListener("input", () => {
  const name = editedName(), area = $("#yaml"), on = onFile(name);
  const draft = drafts.get(name) ?? {base: on};
  draft.text = area.value;
  if (draft.text === draft.base) drafts.delete(name); else drafts.set(name, draft);
  delete $("#saved").dataset.error;
  renderSaving();
  renderList();
});

async function readFile() {
  const answer = await fetch(`${base}/yaml`, {headers: etag ? {"If-None-Match": etag} : {}});
  if (answer.status === 304) return false;
  if (!answer.ok) throw new Error(`project.yaml: ${answer.status}`);
  [file, etag] = [await answer.text(), answer.headers.get("ETag")];
  return true;
}

// One asset's edit put on what the file holds now, by what it held then: an edit elsewhere since stands, and one to these same
// lines asks. A save between this read and the write is refused, and read again.
async function save(name) {
  const draft = drafts.get(name);
  if (!draft) return true;
  for (let tries = 0; tries < 4; tries++) {
    await readFile();
    const now = onFile(name);
    if (now !== draft.base && now !== draft.text) {
      conflict = {name, now};
      renderSaving();
      return false;
    }
    const next = name === WHOLE ? draft.text : write(file, name, draft.text);
    try {
      await ask(`${base}/yaml`, {method: "PUT", headers: {"If-Match": etag}, body: next});
    } catch (error) {
      if (error.code === "project_changed") continue;
      $("#saved").dataset.error = error.message;
      renderSaving();
      return false;
    }
    drafts.delete(name);
    await readFile();
    renderText();
    renderList();
    soon();
    return true;
  }
  return false;
}

async function saveAll() {
  for (const name of [...drafts.keys()]) if (!await save(name)) throw new Error(`${name} isn't saved: see its text`);
}

$("#save").addEventListener("click", () => save(editedName()));
addEventListener("keydown", event => {
  if ((event.ctrlKey || event.metaKey) && event.key === "s") { event.preventDefault(); save(editedName()); }
});
addEventListener("beforeunload", event => { if (drafts.size) event.preventDefault(); });

// ---- Buttons ----

const act = job => async () => {
  $("#said").textContent = "";
  try {
    await job();
  } catch (error) {
    $("#said").textContent = error.message;
  }
};

const render = extra => act(async () => {
  await saveAll();
  await post(`${base}/runs`, {on: [$("#machine").value || "any"], ...extra});
  soon();
});

$("#render").addEventListener("click", () => render({names: [selected]})());
$("#new").addEventListener("click", () => render({names: [selected], new: true, n: +$("#count").value || 1})());
$("#render-all").addEventListener("click", () => render({names: ["all"]})());
$("#keep").addEventListener("click", () => render({names: [selected], take: chosen, in_s: $("#stage video").currentTime})());
$("#approve").addEventListener("click", act(async () => {
  await saveAll();
  await post(`${base}/approve`, {name: selected, version: chosen});
  await readFile();
  await refresh();
}));
$("#unpin").addEventListener("click", act(async () => {
  await saveAll();
  await post(`${base}/unpin`, {name: selected});
  await readFile();
  chosen = null;
  await refresh();
}));
$("#machine").addEventListener("change", () => {
  try { localStorage.setItem("cozy-machine", $("#machine").value); } catch { /* It's only remembered. */ }
  renderActions();
});
$("#whole").addEventListener("click", () => { editing = editing ? null : WHOLE; renderList(); renderText(); });
$("#add").addEventListener("click", act(async () => {
  const name = $("#new-name").value.trim();
  if (!name) return;
  await readFile();
  await ask(`${base}/yaml`, {method: "PUT", headers: {"If-Match": etag}, body: add(file, name, $("#new-kind").value)});
  await readFile();
  $("#new-name").value = "";
  $("#adding").open = false;
  await refresh();
  choose(name);
}));

function renderActions() {
  if (!detail) return;
  const asset = detail.asset, version = shownVersion(), tool = asset.with === "cozy", machine = $("#machine").value;
  $("#render").disabled = !asset.with || (!tool && !machine);
  $("#new").disabled = !asset.with || tool || !machine;
  $("#count").disabled = $("#new").disabled;
  $("#new").textContent = `New ${asset.noun}`;
  $("#approve").disabled = !version || version.approved;
  $("#approve").textContent = version ? `Approve ${asset.noun} ${version.number}` : "Approve";
  $("#unpin").disabled = !asset.pinned || !asset.with;
  $("#keep").hidden = !(version && asset.noun === "take" && !tool && $("#stage video"));
  $("#keep").disabled = !machine;
  $("#render-all").disabled = !machine;
  $("#hint").hidden = !!machine || !asset.with || tool;
}

// ---- Choosing an asset, and staying live ----

function choose(name) {
  editing = null;
  selected = name;
  chosen = null;
  history.replaceState(null, "", `#${encodeURIComponent(name)}`);
  $("#said").textContent = "";
  soon(0);
}

// The version an asset opens on: its state's, else its newest.
function settleChosen() {
  const {asset, versions} = detail;
  if (chosen?.run && (asset.making ?? []).includes(chosen.run)) return;
  if (typeof chosen === "number" && versions.some(version => version.number === chosen)) return;
  const state = main(asset.files);
  const standing = versions.find(version => sameFile(main(version.files), state));
  chosen = cutOf(asset) ? null : (standing ?? versions.at(-1))?.number ?? null;
}

let timer = null, generation = 0;
const soon = (wait = 120) => { clearTimeout(timer); timer = setTimeout(refresh, wait); };

async function refresh() {
  const mine = ++generation;
  const [state] = await Promise.all([ask(base), file ? null : readFile()]);
  if (mine !== generation) return;
  project = state;
  if (!project.assets.some(asset => asset.name === selected)) selected = project.assets[0]?.name ?? null;
  renderTop();
  renderList();
  if (selected) {
    const got = await ask(`${base}/assets/${encodeURIComponent(selected)}`);
    if (mine !== generation) return;
    detail = got;
    settleChosen();
    renderMiddle();
  }
  renderText();
}

function say(kind, words) {
  const stamp = new Date().toLocaleTimeString();
  $("#log").prepend(h(`li${kind ? `.${kind}` : ""}`, {}, `${stamp} ${words}`));
  $("#latest").className = kind;
  $("#latest").textContent = words;
}

function heard(event) {
  switch (event.type) {
    case "started": return say("", `${event.name}: run ${event.number} started`);
    case "landed":
      say("", `${event.name}: ${event.noun} ${event.number} landed`);
      if (event.name === selected && chosen?.run) chosen = event.number;
      return;
    case "said": return say("", event.message);
    case "warning": return say("warning", event.message);
    case "failed": return say("failed", event.error.message);
    case "cut": return say("", `${event.cut}: ${event.unchanged ? "unchanged" : `${event.frames} frames`}`);
    case "ended": return say(event.error ? "failed" : "", event.error ? event.error.message : "The render is finished.");
  }
}

async function tick() {
  if (document.hidden) return;
  if (await readFile().catch(() => false)) soon();
  for (const number of detail?.asset.making ?? []) {
    runs.set(number, await ask(`/v1/runs/${number}`).catch(() => runs.get(number)));
  }
  renderProgress();
}

async function begin() {
  const token = new URLSearchParams(location.hash.slice(1)).get("token");
  if (token) {
    await ask("/v1/session", {method: "POST", headers: {Authorization: `Bearer ${token}`}});
    history.replaceState(null, "", location.pathname);
    selected = null;
  }
  const events = new EventSource(`${base}/events`);
  events.onmessage = message => { heard(JSON.parse(message.data)); soon(); };
  events.onerror = () => ask(base).catch(() => {});
  await refresh();
  await refreshMachines();
  setInterval(() => tick().catch(() => {}), 2000);
  setInterval(() => { if (!document.hidden) refreshMachines().catch(() => {}); }, 8000);
  addEventListener("focus", () => soon());
}

begin().catch(error => ended(error.message));
