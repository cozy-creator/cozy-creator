// The agent API (cl-956, tracker design/agent-api.md) on a real Calcifer, through `cozy` as an agent drives it: a project's nodes listed and
// read whole as a file, every default filled from the tools' own declarations, with takes and the rev each was read at, and what a file can
// hold said in words and as JSON Schema. Then the design's session: a file of nodes refused for a typo with its fix, applied, rewritten and
// applied again as no change; an edit of the page's (e2e/page.py) meanwhile, its words kept and a value both changed a conflict, kept by
// --force-conflicts; words both edited merged by character; the agent's apply and delete each undone from the page. shots.py's Shot renders
// here with no model.
//   node agent.js: on the Calcifer whose lock record is $HOLDER; $COZY: the cozy under test; $ARGV_LOG: each cozy call, for coverage.
// Prints what it saw as JSON and exits 1 if anything failed.
const { execFileSync, spawnSync } = require('child_process');
const { appendFileSync, mkdtempSync, readFileSync, writeFileSync } = require('fs');
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
/** Its rows against the document they were written with (cl-885), after a write: a read takes the rows, so any difference is a fault. */
async function integrity(project, after) {
  const holder = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  const answer = await fetch(`http://${holder.address}/v1/projects/${encodeURIComponent(project)}/integrity`, { headers: { Authorization: `Bearer ${holder.token}` } });
  const found = answer.ok ? (await answer.json()).differences : [`${answer.status} ${await answer.text()}`];
  check(`rows_equal_document_after_${after}`, found.length === 0, found);
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

  // The design's session, in a project of its own.
  const pier = join(homedir(), `web-agent-pier-${Date.now()}`);
  seen.pier = pier;
  check('a_second_project', cozyAt(homedir(), 'new', pier, '--unencrypted').code === 0);
  const at = (...words) => cozyAt(pier, ...words);
  const atJson = (...words) => {
    const ran = at('--json', ...words);
    try { return { code: ran.code, ...JSON.parse(ran.out) }; } catch { return { code: ran.code, out: ran.out, err: ran.err }; }
  };
  const pageAt = (verb, ...words) => execFileSync(PYTHON, [PAGE, verb, pier, ...words], { encoding: 'utf8', timeout: RUN_MS }).trim().split('\n').filter(Boolean).map((line) => JSON.parse(line));
  // Files of its own, so what the first apply sends is theirs.
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=240x136:rate=24:duration=2', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', join(pier, 'look.mp4')]);
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'color=c=blue:size=64x96', '-frames:v', '1', join(pier, 'mara.png')]);
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24:duration=3', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', join(pier, 'look2.mp4')]);
  const file = join(pier, 'pier.yaml');
  const written = `look:
  takes:
    - file: look.mp4
mara:
  kind: image
  description: Mara, in her thirties, red raincoat, short black hair.
  takes:
    - file: mara.png
shot_1:
  ${tool}:
    summary: <mara> walks along <look> at dawn, handheld, one continuous shot.
    beats:
      - duration: 8
        description: <mara> walks toward the camera as gulls wheel overhead.
      - duration: 7
        description: She stops at the railing and looks out to sea.
shot_2:
  ${tool}:
    summary: Close on <mara> at the railing of <look> as the fog lifts.
    beat:
      - duration: 20
        description: She turns toward a ship's bell behind her.
film:
  cozy/edit:
    timeline:
      tracks:
        - clips:
            - clip: <shot_1>
            - clip: <shot_2>
`;
  writeFileSync(file, written);
  const misspelt = at('apply', '-f', file);
  seen.misspelt = misspelt;
  // Off a terminal, uploads say what they sent in one line, with no progress.
  check('uploads_say_one_line', !misspelt.err.includes('Sending files') && /^sent 2 files, [0-9.]+ MiB$/m.test(misspelt.err), misspelt.err);
  check('a_typo_refuses_the_whole_file', misspelt.code === 1 && /^nothing applied: 1 error$/m.test(misspelt.err)
    && misspelt.err.includes(`  shot_2.beat: ${tool} has no field beat. Did you mean beats?`) && at('get').out.startsWith('0 nodes'), misspelt);
  check('diff_says_it_would_refuse', at('diff', '-f', file).code === 2);
  writeFileSync(file, written.replace('    beat:\n', '    beats:\n'));
  const plan = at('diff', '-f', file);
  seen.plan = plan.out;
  check('diff_shows_what_it_would_make', plan.code === 1 && /^\+ look \(video\)\n  \+ takes\.0: take 1, from its file$/m.test(plan.out)
    && /^\+ mara \(image\)\n  \+ description: Mara, in her thirties, red raincoat, short black hair\.$/m.test(plan.out)
    && /^\+ shot_1 \(video\)\n  \+ tool: .*shots\.py\/shoot\n  \+ summary: <mara> walks along <look> at dawn, handheld, one continuous shot\.$/m.test(plan.out)
    && /^\+ film \(edit\)$/m.test(plan.out) && !/^\+ \w+ \(.*(take|description|tool)/m.test(plan.out)
    && /^next: cozy apply -f /m.test(plan.out) && at('get').out.startsWith('0 nodes'), plan);
  const applied = at('apply', '-f', file);
  seen.applied = applied;
  check('no_progress_when_nothing_moves', !applied.err.includes('Sending files'), applied.err);
  check('apply_makes_them', applied.code === 0 && /^created: look \(take 1\), mara \(take 1\), shot_1, shot_2, film$/m.test(applied.out)
    && /warning: shot_2\.beats\.0\.duration: 20 is over 15 s, so 15/.test(applied.err)
    && /pier\.yaml rewritten: each node as it stands, with its rev$/m.test(applied.out) && /^next: cozy render film$/m.test(applied.out), applied);
  const rewritten = readFileSync(file, 'utf8');
  seen.rewritten = rewritten;
  check('the_file_is_rewritten_with_revs', /^mara:\n  kind: image\n  description: Mara, in her thirties, red raincoat, short black hair\.\n  approved: 1\n  takes:\n    - number: 1\n      state: done\n      file: \//m.test(rewritten)
    && (rewritten.match(/^  rev: \"?[0-9a-f]{8}\"?$/gm) ?? []).length === 5 && /^        duration: 15$/m.test(rewritten), rewritten);
  await integrity(pier, 'apply');
  const again = at('apply', '-f', file);
  check('applied_again_it_is_unchanged', again.code === 0 && /^unchanged: look, mara, shot_1, shot_2, film$/m.test(again.out) && !/^next:/m.test(again.out), again);
  const same = at('diff', '-f', file);
  check('diff_of_it_says_no_changes', same.code === 0 && same.out.trim() === 'no changes', same);
  const history = pageAt('history')[0];
  check('one_entry_by_the_cli', history.entries?.[0]?.by?.via === 'cli' && history.entries.filter((entry) => entry.by?.via === 'cli').length === 1, history.entries?.slice(0, 3));

  // Rendered as the page's Render does: what the film reads first.
  const render = at('render', 'film', '--on', 'local');
  const rendered = atJson('get', 'film', 'shot_2');
  seen.render = render;
  await integrity(pier, 'render');
  check('rendered', render.code === 0 && rendered.nodes?.film?.takes?.[0]?.state === 'done' && rendered.nodes?.shot_2?.takes?.[0]?.number === 1, { render, nodes: rendered.nodes });

  // The agent edits shot_2's beat; the page meanwhile sets its length and adds to the summary.
  writeFileSync(file, at('get', 'shot_2').out);
  const read2 = readFileSync(file, 'utf8');
  const edited = read2.replace("      - description: She turns toward a ship's bell behind her.\n        duration: 15", '      - description: A ship\'s bell rings behind her. She turns toward it and smiles.\n        duration: 6');
  writeFileSync(file, edited);
  pageAt('set', 'shot_2', '--fields', JSON.stringify({ beats: [{ description: "She turns toward a ship's bell behind her.", duration: 8 }], summary: 'Close on <mara> at the railing of <look> as the fog lifts, in gold light.' }));
  await integrity(pier, 'page_set');
  const conflicted = at('diff', '-f', file);
  seen.conflicted = conflicted.out;
  check('a_conflict_with_the_page', conflicted.code === 2 && /^shot_2\.beats\.0\.description\n  - She turns toward a ship's bell behind her\.\n  \+ A ship's bell rings behind her\. She turns toward it and smiles\.$/m.test(conflicted.out)
    && /^conflict: shot_2\.beats\.0\.duration: yours 6, the page's 8 since its rev [0-9a-f]{8}$/m.test(conflicted.out)
    && /^next: set 8 and apply, or apply --force-conflicts to keep 6$/m.test(conflicted.out), conflicted);
  const refused = at('apply', '-f', file);
  check('a_conflict_refuses', refused.code === 1 && /^nothing applied: 1 conflict$/m.test(refused.err), refused);
  writeFileSync(file, edited.replace('        duration: 6', '        duration: 8'));
  const settled = at('apply', '-f', file);
  const merged = atJson('get', 'shot_2').nodes?.shot_2?.[tool];
  check('settled_it_applies', settled.code === 0 && /^configured: shot_2 \(beats\.0\.description\)$/m.test(settled.out), settled);
  check('the_page_words_stand', merged?.summary === 'Close on <mara> at the railing of <look> as the fog lifts, in gold light.'
    && merged?.beats?.[0]?.description === "A ship's bell rings behind her. She turns toward it and smiles." && merged?.beats?.[0]?.duration === 8, merged);

  await integrity(pier, 'merged_apply');
  // Undo of the agent's apply, from the page: its words go, the page's stay.
  const undone = pageAt('undo')[0];
  await integrity(pier, 'page_undo');
  const back = atJson('get', 'shot_2').nodes?.shot_2?.[tool];
  check('the_page_undoes_the_apply', back?.beats?.[0]?.description === "She turns toward a ship's bell behind her." && back?.summary?.endsWith('in gold light.'), { undone, back });

  // Words both edited, apart, merge by character; overlapping, they conflict.
  writeFileSync(file, at('get', 'shot_1').out);
  const shot1 = readFileSync(file, 'utf8');
  writeFileSync(file, shot1.replace('    summary: <mara> walks along <look> at dawn,', '    summary: Slowly, <mara> walks along <look> at dawn,'));
  pageAt('set', 'shot_1', '--fields', JSON.stringify({ summary: '<mara> walks along <look> at dawn, handheld, one continuous shot, in fog.' }));
  const textual = at('apply', '-f', file);
  const summary = atJson('get', 'shot_1').nodes?.shot_1?.[tool]?.summary;
  await integrity(pier, 'words_merged');
  check('words_apart_merge', textual.code === 0 && summary === 'Slowly, <mara> walks along <look> at dawn, handheld, one continuous shot, in fog.', { textual, summary });
  writeFileSync(file, at('get', 'shot_1').out.replace('handheld', 'on a tripod'));
  pageAt('set', 'shot_1', '--fields', JSON.stringify({ summary: 'Slowly, <mara> walks along <look> at dawn, steadicam, one continuous shot, in fog.' }));
  const overlapping = at('apply', '-f', file);
  check('words_overlapping_conflict', overlapping.code === 1 && /conflict: shot_1\.summary: yours /.test(overlapping.err), overlapping);
  const forced = at('apply', '-f', file, '--force-conflicts');
  const kept = atJson('get', 'shot_1').nodes?.shot_1?.[tool]?.summary;
  await integrity(pier, 'force_conflicts');
  check('force_conflicts_keeps_yours', forced.code === 0 && kept?.includes('on a tripod') && !kept.includes('steadicam'), { forced, kept });

  // No rev, a node's typo, a take added from a file, approval, and stdin.
  writeFileSync(file, `shot_1:\n  ${tool}:\n    seed: 5\n`);
  const norev = at('apply', '-f', file);
  check('no_rev_is_refused', norev.code === 1 && norev.err.includes('shot_1: no rev; run `cozy get shot_1` first'), norev);
  writeFileSync(file, `mara:\n  kinds: image\n`);
  check('a_node_typo', /mara\.kinds: a node has no field kinds\. Did you mean kind\?/.test(at('apply', '-f', file).err));
  writeFileSync(file, at('get', 'look', 'film').out.replace(/^(look:\n(?:  .*\n)*?  takes:\n)/m, '$1    - file: look2.mp4\n').replace(/^(film:\n(?:  .*\n)*?)  approved: null\n/m, '$1  approved: 1\n'));
  const more = at('apply', '-f', file);
  await integrity(pier, 'take_and_approval');
  check('a_take_from_a_file_and_an_approval', more.code === 0 && /configured: look \(take 2\), film \(approved 1\)/.test(more.out) && !/^next:/m.test(more.out), more);
  const rows2 = atJson('get');
  check('approved_on_the_list', rows2.nodes?.find((row) => row.name === 'film')?.approved === 1 && rows2.nodes?.find((row) => row.name === 'look')?.latest === '2 done', rows2.nodes);
  // The page's Undo of that apply, then the file read before it applied again: the page's change stands, said in one line.
  writeFileSync(file, at('get', 'film').out);
  pageAt('undo');
  const stood = at('apply', '-f', file);
  seen.stood = stood;
  check('a_page_change_the_file_kept_is_said', stood.code === 0 && /^unchanged: film$/m.test(stood.out)
    && /^film\.approved: the page's change since [0-9a-f]{8} stands \(null\); .*pier\.yaml now shows it$/m.test(stood.err)
    && /^  approved: null$/m.test(readFileSync(file, 'utf8')), stood);
  const piped = spawnSync(process.env.COZY, ['apply', '-f', '-'], { cwd: pier, input: at('get', 'shot_1').out, encoding: 'utf8' });
  if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `apply\x1f-f\x1f-\x1f\x1e`);
  check('stdin_prints_the_merged_nodes', piped.status === 0 && /^shot_1:\n  kind: video\n/.test(piped.stdout) && /unchanged: shot_1/.test(piped.stderr), { out: piped.stdout.slice(0, 300), err: piped.stderr });

  // Delete, undone from the page; a name it doesn't have refuses all.
  check('delete_refuses_a_typo', /no node shot_3/.test(at('delete', 'shot_2', 'shot_3').err) && atJson('get').nodes?.length === 5);
  const gone = at('delete', 'shot_2', 'look');
  await integrity(pier, 'delete');
  check('delete_removes_them', gone.code === 0 && /^deleted: shot_2, look$/m.test(gone.out) && atJson('get').nodes?.length === 3, gone);
  pageAt('undo');
  await integrity(pier, 'delete_undone');
  check('the_page_brings_them_back', atJson('get').nodes?.length === 5, atJson('get').nodes);

  // Time downloading weights is no slowness: tool.py's `fetching`, its bar three quick runs, then a run 30 s slower drawing, which
  // warns, and one that spends 30 s in the stage a weight download is first, which doesn't.
  const fetching = `${join(__dirname, '..', 'project', 'tool.py')}/fetching`;
  const warned = (...words) => {
    const ran = at('--json', 'run', fetching, ...words, '--on', 'local');
    const runs = ran.out.split('\n').filter((line) => line.startsWith('{')).map((line) => JSON.parse(line)).filter((line) => line.run?.number);
    const number = runs.at(-1)?.run?.number;
    const record = number ? JSON.parse(at('--json', 'run', 'watch', String(number), '--once').out.split('\n').find((line) => line.startsWith('{')) ?? '{}') : {};
    return { code: ran.code, number, codes: (record.warnings ?? record.run?.warnings ?? []).map((warning) => warning.code), err: ran.err.slice(-400) };
  };
  for (let i = 0; i < 3; i += 1) warned('seconds:=1', 'download:=0');
  const drawn = warned('seconds:=30');
  const fetched = warned('seconds:=1', 'download:=30');
  seen.slow = { drawn, fetched };
  check('a_slow_run_warns', drawn.code === 0 && drawn.codes.includes('slow_run'), drawn);
  check('downloading_weights_is_no_slowness', fetched.code === 0 && !fetched.codes.includes('slow_run'), fetched);
})().catch(async (error) => {
  failures.push(`died: ${error.stack ?? error}`);
}).finally(() => {
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
});
