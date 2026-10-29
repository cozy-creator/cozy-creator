"""Suite `tensorfs`'s verdict, from what cells.sh kept under <gate>/suite-tensorfs (analyze.py runs check(gate) when
the suite is selected). Times are held against the SKU's last passed suite (baseline['suites']['tensorfs']), like the
product cells: the worst known-good plus 25%, unless the known-good spread is wider. Rates are reported beside the
probe's disk ceiling; an executor never dials, so there is no network ceiling to read them against. A PASS appends
this run to the baseline.
    check.py --past <ndjson> <stage> <fraction>   bytes moved once <stage> passed the fraction (cells.sh's tfs_cut)"""
import json
import os
import sys
import time

SLACK = 0.25
# Block 0's and refiner 0's projections of MiniMaxAI/MiniMax-H3@42ed227e's Ref2VA DiT as h3.native reads them (each
# head's [q k v] gathered, fc1's halves swapped): sha256 of their bf16 bytes. Comfy-Org's repack is those bytes in
# ComfyUI's layout ([q; k; v] whole), so its import must give exactly these.
H3_OFFICIAL = {
    'transformer_blocks.0.attn.to_q.weight': 'd2a366742ee3018c728d43db72b5ac0c56161d417e1443766c59af7cc250f2d8',
    'transformer_blocks.0.attn.to_k.weight': 'a1cb573cd85db86b67ccbd90fcad505e05235b5045fb59ba951b8a4f4341444d',
    'transformer_blocks.0.attn.to_v.weight': 'f879ff97e05685c5737b450132c8692d380f5f3789bd6eeda2e2fe3e6a17b22a',
    'transformer_blocks.0.ff.net.0.proj.weight': 'e7cf97ae56a095d37b98a17fc4da01df7aaee6ef022fa045d02530038cae3b19',
    'token_refiner.refiner_blocks.0.attn.to_q.weight': '2d84f6fbfee39b4c0c07b2d8528e27b33ece946bd0f785bc462119102bb38109',
    'token_refiner.refiner_blocks.0.attn.to_k.weight': '9327d52f06cb66ed5c74db26e4034e5b3ce8a2284246cda114f81f066704d370',
    'token_refiner.refiner_blocks.0.attn.to_v.weight': '9316a158bdc07d981352c3f61992756f20de9ca30ebe8e01cdeed1f856574d72',
    'token_refiner.refiner_blocks.0.ff.net.0.proj.weight': '8334e1d9153839356e1caba239d318b75e73a71aefe2423c8e5b27542c1293d0',
}


def events(path):
    out = []
    for line in open(path) if os.path.exists(path) else []:
        line = line.strip()
        if line.startswith('{'):
            try:
                out.append(json.loads(line))
            except ValueError:
                pass
    return out


def stage_progress(evs, stage):
    """(bytes done, bytes total) of the last progress in a stage naming <stage>."""
    last = (0, 0)
    for e in evs:
        p = e.get('progress') or {}
        if e.get('type') == 'run.progress' and stage in (p.get('stage') or ''):
            last = (p.get('bytes_done') or 0, p.get('bytes_total') or 0)
    return last


def stage_seconds(evs, stage):
    """From the log line that opened <stage> to the run's end."""
    start = next((e['at_ms'] for e in evs if e.get('type') == 'run.log' and stage in (e.get('text') or '')), None)
    end = next((e['run'].get('settled_at_ms') for e in reversed(evs) if isinstance(e.get('run'), dict)
                and e['run'].get('settled_at_ms')), None)
    return (end - start) / 1000 if start and end else None


def hub_bytes(rec):
    """What the run fetched from the Hub and the volume: every `bytes: {hub, volume}` its result carries."""
    found = landed_bytes(rec)
    return (sum(b.get('hub') or 0 for b in found), sum(b.get('volume') or 0 for b in found)) if found else (None, None)


def landed_bytes(rec):
    """Every `bytes: {hub, volume, local}` the run's result carries."""
    found = []

    def walk(o):
        if isinstance(o, dict):
            b = o.get('bytes')
            if isinstance(b, dict) and 'hub' in b:
                found.append(b)
            for v in o.values():
                walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)
    walk((rec or {}).get('result'))
    return found


def check(gate):
    t = os.path.join(gate['out'], 'suite-tensorfs')
    if not os.path.isdir(t):
        return [], []
    fails, notes, rows = [], [], {}
    for line in open(os.path.join(t, 'steps.tsv')) if os.path.exists(os.path.join(t, 'steps.tsv')) else []:
        name, number, wall = (line.rstrip('\n').split('\t') + ['', ''])[:3]
        p = os.path.join(t, name + '.json')
        rows[name] = {'run': number, 'wall_s': int(wall) / 1000 if wall else None,
                      'rec': json.load(open(p)) if os.path.exists(p) else None, 'evs': events(os.path.join(t, name + '.ndjson'))}

    # The cases cells.sh ran (--only selects some); none listed: every case.
    path = os.path.join(t, 'cases')
    cases = set(open(path).read().split()) if os.path.exists(path) else None

    def chosen(name):
        case = name.split('-')[0]
        return cases is None or {'mc': 'multi-conversion', 'lora': 'lora-alpha', 'layout': 'family', 'h3': 'h3-comfy'}.get(case, case) in cases

    def done(name, cancelled=False):
        if not chosen(name):
            return False
        r = rows.get(name)
        status = ((r or {}).get('rec') or {}).get('status')
        if r is None:
            fails.append(f'tensorfs {name}: did not run')
            return False
        if cancelled:
            return True
        if status != 'completed':
            why = ((r or {}).get('rec') or {}).get('reason')
            fails.append(f'tensorfs {name}: run {(r or {}).get("run")} {status} {json.dumps(why)[:300] if why else ""}')
            return False
        return True

    base = gate['baseline'].setdefault('suites', {}).setdefault('tensorfs', {})
    now = {}

    def against(case, seconds, nbytes, what):
        if seconds is None:
            return
        now[case] = {'seconds': round(seconds, 2), 'bytes': nbytes}
        good = [g for g in base.get(case, []) if not nbytes or not g.get('bytes') or abs(g['bytes'] - nbytes) <= 0.05 * nbytes]
        if not good:
            notes.append(f'tensorfs {case}: {what} {seconds:.1f} s, no known-good yet: this run starts them')
            return
        vals = [g['seconds'] for g in good]
        limit = max(vals) + max(SLACK * max(vals), max(vals) - min(vals))
        (fails if seconds > limit else notes).append(
            f'tensorfs {case}: {what} {seconds:.1f} s vs known-good {min(vals):.1f}-{max(vals):.1f} s (limit {limit:.1f})')

    if done('probe'):
        d = rows['probe']['rec'].get('result') or {}
        notes.append(f'tensorfs probe: the machine\'s disk writes {(d.get("disk_write_bytes_per_s") or 0) / 1e6:.0f} MB/s, '
                     f'reads cold {(d.get("disk_read_bytes_per_s") or 0) / 1e6:.0f} MB/s')
    st = {}
    for line in open(os.path.join(t, 'straggler.tsv')) if os.path.exists(os.path.join(t, 'straggler.tsv')) else []:
        name, code, ms = line.rstrip('\n').split('\t')
        proxy = events(os.path.join(t, name + '.proxy'))
        objs = [r for r in proxy if r.get('kind') == 'object']
        st[name] = {'ok': code == '0', 'seconds': int(ms) / 1000, 'slowed': sum(1 for r in objs if r['slow']),
                    'reasked': sum(1 for r in objs if r['ask'] > 1), 'bytes': sum(r['bytes'] for r in objs),
                    'error': open(os.path.join(t, name + '.err')).read()[-300:] if code != '0' else ''}
    c, sl = st.get('straggler-clean'), st.get('straggler-slow')
    if not chosen('straggler'):
        pass
    elif not c or not sl:
        fails.append('tensorfs straggler: the case did not run')
    elif not c['ok'] or not sl['ok']:
        fails.append(f'tensorfs straggler: tfs download failed (clean: {c["error"]}; with stragglers: {sl["error"]})')
    elif not sl['slowed']:
        fails.append('tensorfs straggler: no object was slowed: the case tested nothing')
    else:
        limit = 1.5 * c['seconds'] + 20
        (fails if sl['seconds'] > limit else notes).append(
            f'tensorfs straggler: {sl["slowed"]} slow objects, {sl["reasked"]} re-asked; {sl["seconds"]:.1f} s vs clean '
            f'{c["seconds"]:.1f} s (limit {limit:.1f})')
        against('straggler', sl['seconds'], c['bytes'], 'download with stragglers')

    def rate(name, nbytes, seconds):
        if nbytes and seconds:
            notes.append(f'tensorfs {name}: {nbytes / seconds / 1e6:.0f} MB/s')

    if done('download-cold'):
        hub, vol = hub_bytes(rows['download-cold']['rec'])
        total = (hub or 0) + (vol or 0)
        # A datacenter's model cache serves a "cold" download as volume bytes: its times are its own baseline.
        src = 'hub' if hub and hub >= total / 2 else 'cache'
        if not total:
            fails.append('tensorfs download-cold: nothing was downloaded (the store held it already?)')
        rate('download-cold', total, rows['download-cold']['wall_s'])
        notes.append(f'tensorfs download-cold: {(hub or 0) / 2**30:.1f} GiB from the Hub, {(vol or 0) / 2**30:.1f} GiB from the volume')
        against(f'download-cold-{src}', rows['download-cold']['wall_s'], total, f'{total / 2**30:.1f} GiB ({src})')
    if done('download-again'):
        hub, _ = hub_bytes(rows['download-again']['rec'])
        if hub:
            fails.append(f'tensorfs download-again: {hub / 2**20:.0f} MiB came from the Hub again: the store did not hold it')
    if done('download-cut', cancelled=True) and done('download-resume'):
        # Judged against the model, not against what the resume fetched: it must find what the cut run
        # landed (`local`) and fetch only the rest of the model's bytes.
        cut = os.path.join(t, 'download-cut.cut')
        moved = int(open(cut).read()) if os.path.exists(cut) else 0
        rec = rows['download-resume']['rec']
        hub, vol = hub_bytes(rec)
        found = landed_bytes(rec)
        local = sum(b.get('local') or 0 for b in found) if found and all('local' in b for b in found) else None
        _, total = stage_progress(rows['download-resume']['evs'], 'downloading')
        fetched = (hub or 0) + (vol or 0)
        mib = lambda n: f'{(n or 0) / 2**20:.0f} MiB'
        if not moved:
            fails.append('tensorfs download-resume: the first download was never cut part-way: nothing was resumed')
        elif local is None or not total:
            fails.append('tensorfs download-resume: its record carries no held (`local`) bytes or no total: not judged')
        elif local < 0.8 * min(moved, total) or fetched > total - local + 64 * 2**20:
            fails.append(f'tensorfs download-resume: held {mib(local)} of {mib(total)} after {mib(moved)} had landed, '
                         f'then fetched {mib(fetched)}: it started over')
        else:
            notes.append(f'tensorfs download-resume: held {mib(local)} of {mib(total)} ({mib(moved)} had landed by the cut), '
                         f'fetched only the rest: {mib(hub)} from the Hub, {mib(vol)} from the volume'
                         + ('' if hub else ' (the cache served it: the Hub path was not exercised)'))
    if done('h3-comfy-download') and done('h3-comfy-layout'):
        got = {h.get('key'): h.get('sha256') for h in (rows['h3-comfy-layout']['rec'].get('result') or {}).get('hashes') or []}
        wrong = sorted(k for k, v in H3_OFFICIAL.items() if got.get(k) != v)
        if wrong:
            fails.append(f'tensorfs h3-comfy-layout: {len(wrong)} of {len(H3_OFFICIAL)} projections are not the official '
                         f'bytes ({", ".join(wrong[:4])}): ComfyUI\'s H3 DiT imported in the wrong layout renders grey')
        else:
            notes.append(f'tensorfs h3-comfy-layout: ComfyUI\'s H3 DiT imports to the official bytes ({len(H3_OFFICIAL)} projections)')
    up = None
    if done('upload'):
        evs = rows['upload']['evs']
        sec, (_, total) = stage_seconds(evs, 'uploading to'), stage_progress(evs, 'uploading to')
        up = (total / sec) if sec and total else None
        rate('upload', total, sec)
        against('upload', sec, total, f'{total / 2**20:.0f} MiB sent')
    if done('upload-again'):
        wall, first = rows['upload-again']['wall_s'], (rows.get('upload') or {}).get('wall_s') or 0
        if wall is None or wall > max(15, 0.1 * first):
            fails.append(f'tensorfs upload-again: {wall} s to send what the Hub holds (the first took {first:.0f} s): no dedup')
        else:
            notes.append(f'tensorfs upload-again: {wall:.1f} s for content the Hub held (the first took {first:.0f} s)')
    if done('upload-cut', cancelled=True) and done('upload-resume'):
        cut = os.path.join(t, 'upload-cut.cut')
        moved = int(open(cut).read()) if os.path.exists(cut) else 0
        evs = rows['upload-resume']['evs']
        sec, (_, total) = stage_seconds(evs, 'uploading to'), stage_progress(evs, 'uploading to')
        if not moved or not sec or not total or not up:
            fails.append(f'tensorfs upload-resume: cut at {moved} bytes, resumed in {sec} s: nothing to judge')
        else:
            limit = 1.5 * (total - 0.8 * moved) / up + 15
            (fails if sec > limit else notes).append(
                f'tensorfs upload-resume: {sec:.1f} s for the rest after {moved / 2**20:.0f} of {total / 2**20:.0f} MiB '
                f'(limit {limit:.1f} s at the upload\'s own rate)')
    if chosen('lora-alpha'):
        p = os.path.join(t, 'lora-alpha.json')
        try:
            (cfg,) = [c['config'] for c in json.load(open(p))['components'] if c.get('config', {}).get('peft_type') == 'LORA']
        except (OSError, ValueError, KeyError):
            fails.append(f'tensorfs lora-alpha: no LoRA component was imported (see {t}/lora-alpha/import.txt)')
        else:
            if cfg.get('lora_alpha') != cfg.get('r'):
                fails.append(f'tensorfs lora-alpha: committed lora_alpha {cfg.get("lora_alpha")} at r {cfg.get("r")}: kohya\'s '
                             'ss_network_alpha set the scale, which no loader of a file without .alpha tensors does')
            else:
                notes.append(f'tensorfs lora-alpha: lora_alpha {cfg["lora_alpha"]} = r, whatever the metadata says')
    # The lane's commit holds both components' weights encoded (inspect also held them to the source).
    if all(done(name) for name in ('mc-source', 'mc-upload', 'mc-quantize', 'mc-inspect')):
        held = (rows['mc-inspect']['rec'].get('result') or {}).get('tensors') or []
        encoded = sorted({h['component'] for h in held if h.get('scheme') == 'int8.convrot'})
        worst = max((h['relative_error'] for h in held if h.get('relative_error') is not None), default=0)
        ok = encoded == ['decoder', 'encoder']
        (notes if ok else fails).append(
            f'tensorfs multi-conversion: quantize run {rows["mc-quantize"]["run"]}, one call per component: '
            f'{", ".join(encoded) or "no component"} encoded{"" if ok else " (want decoder, encoder)"}; '
            f'worst relative error {worst:.4f}')
    # Refused at ingest, before the bulk downloads (tfs-137): the refusal's code, and what moved before it.
    for name, code, cap in (('family-refusal', 'FAMILY_MISMATCH', 64 << 20), ('layout-probe', 'LAYOUT_MISMATCH', 512 << 20)):
        if not chosen(name):
            continue
        r = rows.get(name)
        if r is None:
            fails.append(f'tensorfs {name}: did not run')
            continue
        rec = r['rec'] or {}
        why = json.dumps(rec.get('reason') or '')
        moved = max((stage_progress([e], 'download')[0] for e in r['evs']), default=0)
        if rec.get('status') != 'failed' or code not in why:
            fails.append(f'tensorfs {name}: run {r["run"]} {rec.get("status")}, not refused with {code}: {why[:300]}')
        elif moved > cap:
            fails.append(f'tensorfs {name}: {moved / 2**20:.0f} MiB downloaded before the refusal (at most {cap >> 20})')
        else:
            notes.append(f'tensorfs {name}: refused ({code}) after {moved / 2**20:.1f} MiB, {r["wall_s"]} s')
    if not fails:
        label = open(os.path.join(gate['out'], 'label')).read().strip()
        at = time.strftime('%Y-%m-%dT%H:%MZ', time.gmtime())
        for case, v in now.items():
            base.setdefault(case, []).append({**v, 'label': label, 'at': at})
    return fails, notes


if __name__ == '__main__' and sys.argv[1:2] == ['--past']:
    done_, total = stage_progress(events(sys.argv[2]), sys.argv[3])
    if total and done_ >= float(sys.argv[4]) * total:
        print(done_)
