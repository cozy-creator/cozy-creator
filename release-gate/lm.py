"""LM coverage: the greedy (temperature 0) token loops the image and video packages run inside a request ran in this
gate, not skipped or memoized: Fibo's and Klein's prompt upsamplers, and H3's reference describer (Qwen3.5).
    check(gate) -> (fails, notes)"""
import json


def check(gate):
    fails, notes = [], []
    for name in ('fibo', 'klein'):
        run = gate['runs'].get(name)
        if run is None:
            continue
        ms = run.stage_ms('upsampling prompt')
        if ms:
            notes.append(f'lm: {name} prompt upsampler ran, greedy, {ms / 1000:.1f} s')
        else:
            fails.append(f'{name}: its prompt upsampler did not run (no `upsampling prompt` stage)')
    run = gate['runs'].get('h3')
    if run is not None:
        # The describer is a Qwen3.5 call or stage of the H3 run; its trace names it.
        text = json.dumps({k: run.record.get(k) for k in ('calls', 'measurements')}).lower()
        if 'qwen3.5' in text or 'qwen3_5' in text or 'describ' in text:
            notes.append('lm: h3 described its character reference with Qwen3.5, greedy')
        else:
            fails.append('h3: its character reference was not described (no Qwen3.5 call or describe stage)')
    return fails, notes
