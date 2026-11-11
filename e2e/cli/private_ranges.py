#!/usr/bin/env python3
"""Exercise both range binders with one mounted synthetic video on Linux."""
import argparse
import hashlib
from http.client import HTTPConnection
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from urllib.parse import quote, urlencode
import uuid

from private_execution import contains, observed_run


def digest(path):
    with path.open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()


def clips(directory):
    if not directory.exists():
        return {}
    return {path.name: digest(path) for path in directory.iterdir() if path.is_file()}


def video(path):
    reply = subprocess.run(
        ['ffprobe', '-v', 'error', '-select_streams', 'v:0', '-count_frames',
         '-show_entries', 'stream=width,height,r_frame_rate,nb_read_frames,duration',
         '-of', 'json', str(path)], capture_output=True,
    )
    assert reply.returncode == 0, 'ffprobe could not read the fixture video'
    stream, = json.loads(reply.stdout)['streams']
    assert stream['r_frame_rate'] == '24/1', 'the video is not 24 frames per second'
    return {'width': stream['width'], 'height': stream['height'],
            'frames': int(stream['nb_read_frames']),
            'duration_ms': round(float(stream['duration']) * 1000)}


def wait_for(read, ready, failure):
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        value = read()
        if ready(value):
            return value
        time.sleep(0.25)
    raise AssertionError(failure)


def exercise(args, home, receipt):
    project = args.project.resolve(strict=True)
    view = args.view.resolve(strict=True)

    def cli(*argv):
        reply = subprocess.run([str(args.cli), '--json', *argv], cwd=project, capture_output=True)
        receipt['last_cli_exit'] = reply.returncode
        assert reply.returncode == 0, 'the installed CLI refused the range fixture command'
        return reply.stdout

    receipt['stage'] = 'mount'
    status = json.loads(cli('vault', 'status'))
    assert status['encrypted'] and status['state'] == 'unlocked', 'the range fixture needs an unlocked vault'
    assert Path(status['view']).samefile(view), 'the fixture view is not the current mounted project'
    project_id = json.loads(cli('--full', 'status'))['project']['id']
    route = '/v1/projects/' + quote(project_id, safe='')
    holder = json.loads((home / 'calcifer/calcifer.lock.holder').read_text(encoding='utf-8'))
    host, port = holder['address'].rsplit(':', 1)
    assert host in ('127.0.0.1', '[::1]'), 'the fixture needs the default loopback controller'

    def api(method, path, body=None, headers=None, expected=None):
        fields = {'Authorization': 'Bearer ' + holder['token'], **(headers or {})}
        if isinstance(body, dict):
            body = json.dumps(body).encode('utf-8')
            fields['Content-Type'] = 'application/json'
        if method == 'POST':
            fields['Idempotency-Key'] = uuid.uuid4().hex
        connection = HTTPConnection(host.strip('[]'), int(port), timeout=30)
        try:
            connection.request(method, path, body=body, headers=fields)
            reply = connection.getresponse()
            receipt['last_http_status'] = reply.status
            accepted = reply.status == expected if expected is not None else 200 <= reply.status < 300
            assert accepted, 'the controller answered with an unexpected status for the range fixture'
            return json.loads(reply.read())
        finally:
            connection.close()

    receipt['stage'] = 'scoped_import'
    source = view / 'files/movie.mp4'
    source_meta = video(source)
    assert source_meta == {'width': 320, 'height': 180, 'frames': 48, 'duration_ms': 2000}, 'the source is not the fixture two-second video'
    data = source.read_bytes()
    source_hash = 'sha256:' + hashlib.sha256(data).hexdigest()
    receipt['source'] = {'sha256': source_hash, **source_meta}
    upload = '/v1/project-files/' + quote(source_hash, safe='') + '?' + urlencode({'project': project_id})
    held = api('PUT', upload, data, {'Content-Type': 'application/octet-stream',
                                   'Content-Range': f'bytes 0-{len(data) - 1}/{len(data)}'})
    assert held['digest'] == source_hash and held['length'] == len(data), 'the scoped upload did not hold the source bytes'
    imported = api('POST', route + '/imports', {'source': source_hash, 'name': 'vault_range_source', 'exact': True})
    assert imported['name'] == 'vault_range_source' and imported['sha256'] == source_hash, 'the scoped upload did not become the source asset'
    wait_for(lambda: api('GET', route + '/assets/vault_range_source')['asset'],
             lambda asset: asset.get('duration_ms') == source_meta['duration_ms'],
             'the source asset never became measured')

    receipt['stage'] = 'prepare_ranges'
    script = view / 'need.py'
    fixture = Path(__file__).parent / 'project'
    source = (fixture / 'tool.py').read_text(encoding='utf-8')
    decorator = '@app.tool(label="Keep a video")'
    assert source.count(decorator) == 1, 'the range tool fixture changed its declaration'
    source = source.replace(decorator, '@app.tool(label="Keep a video", ui="ui/private.js")')
    marker = f'XS160_TOOL_DISCOVERY_{uuid.uuid4().hex}'
    with script.open('x', encoding='utf-8') as target:
        target.write(source + f'\n# {marker}\n')
    module = view / 'ui/private.js'
    module.parent.mkdir()
    shutil.copyfile(fixture / 'boxed/e2e_boxed/ui/shade.js', module)
    tool = str(script) + '/need'
    private_cache = view / '.vault-calcifer/clips'
    global_cache = home / 'cache/clips'
    before_global = clips(global_cache)
    receipt['global_before'] = len(before_global)

    def check_clip(label, before_private, number):
        current_global = clips(global_cache)
        current_private = clips(private_cache)
        added = current_private.keys() - before_private.keys()
        cell = receipt[label] = {'run': number, 'private_added': len(added),
                                 'global_after': len(current_global),
                                 'global_unchanged': current_global == before_global}
        assert cell['global_unchanged'], 'the binding changed the global clip cache'
        assert len(added) == 1, 'the binding did not create exactly one new private clip'
        name, = added
        clip = (private_cache / name).resolve(strict=True)
        assert clip.is_relative_to(view), 'the clip resolved outside the mounted vault'
        clip_hash = 'sha256:' + current_private[name]
        cell.update(clip_sha256=clip_hash, clip_name_sha256=hashlib.sha256(name.encode()).hexdigest(),
                    clip_bytes=clip.stat().st_size, **video(clip))
        assert cell['frames'] == 24 and 900 <= cell['duration_ms'] <= 1100, 'the cached clip is not the selected one-second range'
        assert (cell['width'], cell['height']) == (320, 180) and clip_hash != source_hash, 'the cached range is not a cut of the source'
        run = api('GET', f'/v1/runs/{number}?detail=full')
        cell.update(project_id_matches=run.get('project_id') == project_id,
                    local_machine=run.get('machine') == 'local', run_status=run.get('status'))
        assert cell['project_id_matches'], 'the run did not retain the canonical project identity'
        assert cell['local_machine'], 'the run did not use the local machine'
        assert cell['run_status'] == 'completed', 'the actual project run did not complete'
        assert run['request']['input']['video'] == clip_hash, 'the run did not bind the new private clip digest'
        output, = [item for item in run['output'] if item['name'] == 'video']
        assert output['status'] == 'completed' and output.get('sha256'), 'the local function did not land its video'
        cell['output_sha256'] = output['sha256']
        return current_private, name

    receipt['stage'] = 'canvas_range'
    before_private = clips(private_cache)
    asset = 'vault_range_render'
    made = api('POST', route + '/assets', {'tool': tool, 'name': asset, 'kind': 'video', 'exact': True,
                                         'fields': {'video': '<vault_range_source:0-24>'}})
    assert made['name'] == asset, 'the render fixture asset was not created'
    receipt['stage'] = 'cold_private_tool_discovery'
    tools = api('GET', '/v1/tools?' + urlencode({'project': project_id}))['tools']
    assert any(row['id'] == tool and row.get('local') and row.get('ui') for row in tools), 'cold discovery did not return the private tool and its declared UI'
    copies = view / '.vault-calcifer/code'
    shared_code = home / 'calcifer/code'
    markers = [marker.encode()]
    assert any(contains(path, markers) for path in copies.rglob('*.py')), 'private discovery did not stage the tool in its vault'
    assert not any(contains(path, markers) for path in shared_code.rglob('*.py')), 'private discovery copied source into the shared code cache'
    module_route = '/v1/tools/' + quote(tool, safe='') + '/ui?' + urlencode({'project': project_id})
    refused = api('GET', module_route, expected=404)
    assert refused.get('error', {}).get('code') == 'tool_ui_none', 'the private tool module did not select the declared-control fallback'
    receipt['private_tool_discovery'] = {'staged_in_vault': True, 'shared_code_unchanged': True, 'module_refused': True}
    receipt['stage'] = 'canvas_range'
    asked = api('POST', route + '/runs', {'names': [asset], 'on': ['local']})
    assert asked['id'] == project_id and asked['added'] == 1, 'the canvas did not queue one render'

    def render_state():
        state = api('GET', route + '/assets/' + asset)
        assert not state['asset'].get('failure'), 'the canvas range render failed'
        return state

    landed = wait_for(render_state, lambda state: bool(state['versions']), 'the canvas range render never landed')
    version, = landed['versions']
    number, = version['runs']
    before_cli, canvas_name = check_clip('canvas', before_private, number)
    receipt['canvas']['version'] = version['number']

    receipt['stage'] = 'standalone_range'
    call = observed_run(cli('run', tool, 'video=<vault_range_source:24-48>', '--on', 'local'))
    after_cli, cli_name = check_clip('standalone', before_cli, call['number'])
    assert cli_name != canvas_name and after_cli.get(canvas_name) == before_cli[canvas_name], 'the second binding did not retain a distinct private clip'
    receipt['passed'] = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', required=True, type=Path)
    parser.add_argument('--view', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--cli', required=True, type=Path)
    args = parser.parse_args()
    runner = os.environ.get('RUNNER_TEMP')
    home = Path.home() / '.cozy'
    if (sys.platform != 'linux' or os.environ.get('GITHUB_ACTIONS') != 'true' or not runner
            or 'COZY_HOME' in os.environ or not args.out.resolve().is_relative_to(Path(runner).resolve())
            or args.cli.resolve() != (home / 'bin/cozy').resolve()):
        raise SystemExit('range custody requires the installed CLI on a fresh Linux CI runner')
    receipt = {'passed': False}
    try:
        exercise(args, home, receipt)
    except Exception as error:
        reason = str(error) if isinstance(error, AssertionError) else type(error).__name__
        receipt['failure'] = reason
        raise SystemExit(f'private range custody failed at {receipt.get("stage", "setup")}: {reason}') from None
    finally:
        args.out.write_text(json.dumps(receipt, indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
