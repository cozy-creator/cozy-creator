// The agent API (cl-956, tracker design/agent-api.md) on a real Calcifer, through `cozy` as an agent drives it: a project's nodes listed and
// read whole as a file, every default filled from the tools' own declarations, with takes and the rev each was read at, and what a file can
// hold said in words and as JSON Schema. The boxes are made as the page makes them (e2e/page.py); shots.py's Shot renders here with no model.
//   node agent.js: on the Calcifer whose lock record is $HOLDER; $COZY: the cozy under test; $ARGV_LOG: each cozy call, for coverage.
// Prints what it saw as JSON and exits 1 if anything failed.
const { execFileSync, spawnSync } = require('child_process');
const { appendFileSync, mkdtempSync, writeFileSync } = require('fs');
const { homedir, tmpdir } = require('os');
const { join } = require('path');

const failures = [];
const seen = {};
function check(name, ok, detail) {
  seen[name] = ok === true ? true : (detail ?? false);
  if (ok !== true) failures.push(name);
}

const PYTHON = process.platform === 'win32' ? 'python' : 'python3';
const PAGE = join(__dirname, '..', '..', 'page.py');
const SHOTS = join(__dirname, '..', 'project', 'shots.py');
// The first render here installs its tool's environment.
const RUN_MS = 10 * 60_000;
const folder = join(homedir(), `web-agent-${Date.now()}`);
const scratch = mkdtempSync(join(tmpdir(), 'web-agent-'));
seen.project = { folder };

/** One call of the cozy under test from the project, logged for coverage: its exit, stdout and stderr. Never a terminal. */
function cozyAt(cwd, ...words) {
  if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
  const ran = spawnSync(process.env.COZY, words, { cwd, encoding: 'utf8', maxBuffer: 1 << 26, timeout: RUN_MS, stdio: ['ignore', 'pipe', 'pipe'] });
  return { code: ran.status, out: ran.stdout ?? '', err: ran.stderr ?? '' };
}
const cozy = (...words) => cozyAt(folder, ...words);
function json(...words) {
  const ran = cozy('--json', ...words);
  try { return { code: ran.code, ...JSON.parse(ran.out) }; } catch { return { code: ran.code, out: ran.out, err: ran.err }; }
}
/** What the page asks, by e2e/page.py: its JSON lines. */
function page(verb, ...words) {
  const out = execFileSync(PYTHON, [PAGE, verb, folder, ...words], { encoding: 'utf8', timeout: RUN_MS, env: { ...process.env } });
  return out.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line));
}

(async () => {
  const clip = join(scratch, 'look.mp4');
  const picture = join(scratch, 'mara.png');
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24:duration=2', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', clip]);
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'color=c=red:size=64x96', '-frames:v', '1', picture]);
  const made = cozyAt(homedir(), 'new', folder, '--unencrypted');
  check('a_project', made.code === 0, made);

  // Nothing yet: a definite answer, and the next step.
  const none = cozy('get');
  check('an_empty_project_says_0_nodes', none.code === 0 && none.out.split('\n')[0] === '0 nodes' && /^next: /m.test(none.out), none);
  const noneJson = json('get');
  check('as_json_an_empty_list', noneJson.code === 0 && Array.isArray(noneJson.nodes) && noneJson.nodes.length === 0, noneJson);
  const takesAlone = cozy('get', '--takes');
  check('takes_needs_a_name', takesAlone.code === 2 && /cozy get NAME --takes/.test(takesAlone.err), takesAlone);

  // Boxes as the page makes them: two files, a Shot of shots.py, a text and an Edit.
  page('import', clip, '--name', 'look');
  page('import', picture, '--name', 'mara', '--description', 'Mara, in her thirties, red raincoat.');
  page('add', 'shot_1', '--tool', `${SHOTS}/shoot`, '--fields', JSON.stringify({ summary: '<mara> walks along <look>.', beats: [{ description: '<mara> walks toward the camera.', duration: 8 }] }));
  const boxes = join(scratch, 'boxes.json');
  writeFileSync(boxes, JSON.stringify([
    { name: 'style', fields: { text: 'Hand-painted, cel-shaded.' } },
    { name: 'film', tool: 'cozy/edit', fields: { timeline: { tracks: [{ clips: [{ clip: '<shot_1>' }, { clip: '<look>' }] }] } } },
  ]));
  page('boxes', boxes);

  // One line per node, and what follows.
  const listed = cozy('get');
  const rows = Object.fromEntries(listed.out.split('\n').slice(1).filter((line) => line && !line.startsWith('next:')).map((line) => [line.split(/\s+/)[0], line.split(/\s+/).slice(1)]));
  seen.list = listed.out;
  check('one_line_per_node', listed.code === 0 && /^NAME\s+KIND\s+TOOL\s+APPROVED\s+LATEST/.test(listed.out)
    && rows.mara?.[0] === 'image' && rows.look?.[0] === 'video' && rows.shot_1?.[0] === 'video' && rows.style?.[0] === 'text' && rows.film?.[0] === 'edit'
    && /^next: cozy get NAME prints every field$/m.test(listed.out), listed);
  const list = json('get');
  const mara = list.nodes?.find((row) => row.name === 'mara');
  check('the_list_as_json', mara?.kind === 'image' && mara?.approved === 1 && mara?.latest === '1 done' && list.nodes.length === 5, list);

  // A node whole: its kind and words, its tool block with every default, approved, takes and its rev, in that order.
  const one = json('get', 'shot_1');
  const shot = one.nodes?.shot_1;
  const tool = `${SHOTS}/shoot`;
  const block = shot?.[tool];
  seen.shot_1 = shot;
  check('its_keys_in_order', JSON.stringify(Object.keys(shot ?? {})) === JSON.stringify(['kind', 'description', tool, 'approved', 'takes', 'rev']), Object.keys(shot ?? {}));
  check('every_field_with_its_default', shot?.kind === 'video' && block?.summary === '<mara> walks along <look>.' && block?.beats?.[0]?.duration === 8
    && block?.beats?.[0]?.description === '<mara> walks toward the camera.' && block?.plate === null && block?.seed === null && block?.refuse === ''
    && JSON.stringify(block?.loras) === '[]' && shot?.approved === null && JSON.stringify(shot?.takes) === '[]', block);
  check('a_rev', /^[0-9a-f]{8}$/.test(shot?.rev ?? '') && one.rev === shot.rev, one.rev);
  const files = json('get', 'mara', 'style', 'film');
  const take = files.nodes?.mara?.takes?.[0];
  check('a_file_is_its_first_take_approved', files.nodes?.mara?.approved === 1 && take?.number === 1 && take?.state === 'done' && take?.file?.startsWith(folder)
    && files.nodes?.mara?.description === 'Mara, in her thirties, red raincoat.', files.nodes?.mara);
  check('a_text_node_has_its_words', files.nodes?.style?.kind === 'text' && files.nodes?.style?.text === 'Hand-painted, cel-shaded.' && !('takes' in (files.nodes?.style ?? {})), files.nodes?.style);
  const clips = files.nodes?.film?.['cozy/edit']?.timeline?.tracks?.[0]?.clips;
  check('an_edit_has_its_timeline', files.nodes?.film?.kind === 'edit' && clips?.[0]?.clip === '<shot_1>' && clips?.[1]?.clip === '<look>', files.nodes?.film);

  // As YAML: block style, a ratio quoted, the rev last.
  const yaml = cozy('get', 'shot_1', 'mara', 'style');
  seen.yaml = yaml.out;
  check('as_a_file', yaml.code === 0 && /^shot_1:\n  kind: video\n  description: ""\n/.test(yaml.out) && /^    beats:\n      - description: <mara> walks toward the camera\.\n        duration: 8$/m.test(yaml.out)
    && /^  rev: \"?[0-9a-f]{8}\"?$/m.test(yaml.out) && /^mara:\n  kind: image\n/m.test(yaml.out) && /^  text: Hand-painted, cel-shaded\.$/m.test(yaml.out), yaml.out);

  // A name it doesn't have: refused in one line, with the name it likely meant.
  const typo = cozy('get', 'shot');
  check('a_typo_is_refused_with_its_fix', typo.code !== 0 && /did you mean shot_1\?/.test(typo.err) && typo.err.trim().split('\n').length <= 2, typo);

  // Takes: three rendered, the newest shown and the rest counted; every one with --takes, and one by its number.
  page('render', '--on', 'local', '--n', '3', 'shot_1');
  const three = json('get', 'shot_1');
  check('the_newest_take_and_the_rest_counted', JSON.stringify(three.nodes?.shot_1?.takes?.map((held) => held.number)) === '[3]' && three.older?.shot_1 === 2
    && three.nodes.shot_1.takes[0].file?.endsWith('.mp4'), three);
  const counted = cozy('get', 'shot_1');
  check('its_comment_counts_them', /^  takes:\n    # 2 older takes: cozy get shot_1 --takes\n    - number: 3$/m.test(counted.out), counted.out);
  const every = json('get', 'shot_1', '--takes');
  check('takes_lists_every_take', JSON.stringify(every.nodes?.shot_1?.takes?.map((held) => held.number)) === '[1,2,3]' && !every.older?.shot_1, every.nodes?.shot_1?.takes);
  const second = json('get', 'shot_1.2');
  check('name_dot_n_is_that_take', JSON.stringify(second.nodes?.shot_1?.takes?.map((held) => held.number)) === '[2]', second.nodes?.shot_1?.takes);
  const all = json('get', '--all');
  check('all_is_every_node', JSON.stringify(Object.keys(all.nodes ?? {})) === JSON.stringify(['look', 'mara', 'shot_1', 'style', 'film']), Object.keys(all.nodes ?? {}));

  // What a file holds, in words and as JSON Schema 2020-12, from the tools' declarations.
  const said = cozy('explain');
  check('explain_says_the_file', said.code === 0 && /^kind\s+image, video, audio, text or edit/m.test(said.out) && /^cozy\/edit\s+/m.test(said.out), said.out);
  const video = cozy('explain', 'video');
  check('explain_a_kind', video.code === 0 && /^video: /.test(video.out) && /cozy\/edit/.test(video.out) === false, video.out);
  // Local code, as the page lists it for this project.
  const local = page('tools')[0].tools.find((held) => held.function === 'shoot')?.id ?? tool;
  seen.local = local;
  const words = cozy('explain', local);
  seen.explain_tool = words.out;
  check('explain_a_tool', words.code === 0 && /^summary\s+text; <name> mentions a node$/m.test(words.out) && /^beats\s+list$/m.test(words.out)
    && /^  duration\s+1-15 s, default 5$/m.test(words.out) && /^loras\s+list of ref and strength$/m.test(words.out), words);
  const member = cozy('explain', `${local}.beats`);
  check('explain_a_field', member.code === 0 && /^.*\.beats: list$/m.test(member.out) && /^description\s+text; <name> mentions a node$/m.test(member.out), member.out);
  const wrong = cozy('explain', `${local}.beat`);
  check('explain_a_typo', wrong.code !== 0 && /has no field beat\. Did you mean beats\?/.test(wrong.err), wrong);
  const schema = json('explain', '--schema');
  const node = schema.$defs?.node;
  check('a_json_schema', schema.$schema === 'https://json-schema.org/draft/2020-12/schema' && node?.additionalProperties === false
    && node?.properties?.rev?.readOnly === true && node?.dependentSchemas?.['cozy/edit']?.properties?.kind?.const === 'edit', { $schema: schema.$schema, node: Object.keys(node?.properties ?? {}) });
  const blockSchema = json('explain', local, '--schema');
  check('a_tool_block_schema', blockSchema.additionalProperties === false && blockSchema.properties?.beats?.items?.properties?.duration?.maximum === 15, blockSchema);
})().catch(async (error) => {
  failures.push(`died: ${error.stack ?? error}`);
}).finally(() => {
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
});
