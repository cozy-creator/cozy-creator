#!/usr/bin/env python3
"""Real project encryption through the default installed cozy, on an ephemeral CI host."""
import argparse
import base64
import difflib
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib

FIRST_COLD_SECONDS = 300


class Proof:
    def __init__(self, artifacts, source):
        if os.environ.get('GITHUB_ACTIONS') != 'true':
            raise RuntimeError('run this default-home proof only in an ephemeral GitHub runner')
        if 'COZY_HOME' in os.environ or 'COZY_PROJECT' in os.environ:
            raise RuntimeError('the proof requires the normal default home and explicit project paths')
        self.home = Path.home() / '.cozy'
        self.binary = self.home / 'bin' / ('cozy.exe' if os.name == 'nt' else 'cozy')
        found = shutil.which('cozy')
        if not found or Path(found).resolve() != self.binary.resolve():
            raise RuntimeError('PATH must select the installed default ~/.cozy/bin/cozy')
        self.out = artifacts.resolve()
        if self.out == self.home or self.home in self.out.parents:
            raise RuntimeError('proof inputs/evidence must live outside ~/.cozy')
        self.out.mkdir(parents=True, exist_ok=False)
        self.credentials = self.out / 'credentials'
        self.credentials.mkdir(mode=0o700)
        self.project = self.out / 'project'
        self.source = source
        self.commands = []
        self.canary = 'COZY_PRIVATE_PROOF_' + secrets.token_hex(24)
        self.secret_values = []
        self.winarm = os.name == 'nt' and os.environ.get('RUNNER_ARCH') == 'ARM64'
        self.fixture_ffmpeg = 'ffmpeg'
        self.cli_env = None
        if self.winarm:
            self.fixture_ffmpeg = shutil.which('ffmpeg')
            if not self.fixture_ffmpeg:
                raise RuntimeError('fixture generation requires its existing system ffmpeg')
            self.cli_env = dict(os.environ)
            self.cli_env['PATH'] = os.pathsep.join(
                entry for entry in os.environ.get('PATH', '').split(os.pathsep)
                if not any((Path(entry.strip('"') or '.') / name).is_file()
                           for name in ['ffmpeg.exe', 'ffprobe.exe']))
            if (shutil.which('ffmpeg', path=self.cli_env['PATH'])
                    or shutil.which('ffprobe', path=self.cli_env['PATH'])):
                raise RuntimeError('the controller test environment still exposes system media tools')
            if (self.home / 'tools' / 'ffmpeg' / '9.0.2').exists():
                raise RuntimeError('the fresh media proof must start without a cached pin')
        self.receipt = {'result': 'running', 'source': source, 'os': sys.platform,
                        'default_home': str(self.home), 'installed_binary': str(self.binary),
                        'synthetic_only': True, 'cells': {}}

    def private_file(self, name, value):
        path = self.credentials / name
        with path.open('x', encoding='utf-8') as handle:
            path.chmod(0o600)
            handle.write(value + '\n')
        self.secret_values.append(value)
        return path

    def redact(self, text):
        for path in self.credentials.iterdir():
            if path.is_file():
                value = path.read_text().strip()
                if value and value not in self.secret_values:
                    self.secret_values.append(value)
        for value in self.secret_values:
            text = text.replace(value, '[credential redacted]')
        return text

    def command(self, *args, json_output=True, refusal=False, project=True, allow_locking=False,
                full=False, timeout=None):
        command = ['cozy'] + (['--json'] if json_output else [])
        if full:
            command += ['--full']
        if project:
            command += ['project', '-p', str(self.project)]
        command += [str(arg) for arg in args]
        began = time.monotonic()
        record = {'args': command[1:], 'state': 'running'}
        self.commands.append(record)
        self.flush()
        print(f'project vault CLI command: {args[0]}', flush=True)
        completed = subprocess.run(command, capture_output=True, env=self.cli_env, timeout=timeout)
        # Export integrity stamps cover exact UTF8 bytes. text=True would silently rewrite
        # Windows CRLF and make an otherwise valid exported graph fail its own digest.
        completed.stdout = completed.stdout.decode('utf-8')
        completed.stderr = completed.stderr.decode('utf-8', errors='replace')
        record.update(state='complete', exit_code=completed.returncode,
                      seconds=round(time.monotonic() - began, 3))
        self.flush()
        if refusal:
            if completed.returncode == 0:
                raise RuntimeError(f'{args[0]} unexpectedly succeeded while access should be refused')
            return completed
        if allow_locking and completed.returncode:
            try:
                answer = json.loads(completed.stdout)
            except ValueError:
                answer = {}
            if answer.get('error', {}).get('code') == 'project_busy':
                state = self.status()['state']
                if state not in ['locking', 'locked']:
                    raise RuntimeError('busy first lock did not revoke project viewing')
                self.receipt['first_manual_lock'] = {'lock_requests': 1, 'response': 'project_busy',
                                                      'initial_state': state,
                                                      'completion': 'automatic_close_observed_by_status'}
                self.flush()
                self.wait_locked('a manual lock did not finish draining accepted requests')
                self.receipt['first_manual_lock']['final_state'] = 'locked'
                self.flush()
                return answer
        if completed.returncode:
            detail = completed.stderr[-4096:]
            if not json_output:
                detail += completed.stdout[-4096:]
            try:
                error = json.loads(completed.stdout).get('error', {})
                if isinstance(error, dict):
                    detail += f" {error.get('code', '')}: {error.get('message', '')}"
            except (ValueError, AttributeError):
                pass
            raise RuntimeError(f'{args[0]} refused ({completed.returncode}): '
                               + self.redact(detail))
        if not json_output:
            return completed.stdout
        result = json.loads(completed.stdout)
        if isinstance(result, dict) and result.get('recovery_secret'):
            raise RuntimeError('recovery secret reached stdout despite explicit recovery output')
        if allow_locking:
            if result.get('vault', {}).get('state') != 'locked':
                raise RuntimeError('successful first lock did not acknowledge locked state')
            self.receipt['first_manual_lock'] = {'lock_requests': 1, 'response': 'locked',
                                                  'initial_state': 'locked', 'final_state': 'locked',
                                                  'completion': 'lock_acknowledgement'}
            self.flush()
        return result

    def flush(self):
        (self.out / 'commands.json').write_text(json.dumps(self.commands, indent=2) + '\n')
        (self.out / 'receipt.json').write_text(json.dumps(self.receipt, indent=2) + '\n')

    def diagnose_mount_holders(self):
        """Capture kernel paths, never process argv/environment or file contents."""
        if not sys.platform.startswith('linux'):
            return
        prefix = str(self.home / 'calcifer' / 'vaults') + '/'
        holders = []
        for process in Path('/proc').iterdir():
            if not process.name.isdecimal():
                continue
            paths = []
            for kind in ['cwd', 'root']:
                try:
                    target = os.readlink(process / kind)
                    if target.startswith(prefix):
                        paths.append({'kind': kind, 'path': target})
                except OSError:
                    pass
            try:
                for descriptor in (process / 'fd').iterdir():
                    try:
                        target = os.readlink(descriptor)
                        if target.startswith(prefix):
                            paths.append({'kind': 'fd', 'fd': descriptor.name, 'path': target})
                    except OSError:
                        pass
                for line in (process / 'maps').read_text().splitlines():
                    parts = line.split(None, 5)
                    if len(parts) == 6 and parts[5].startswith(prefix):
                        paths.append({'kind': 'mapping', 'path': parts[5]})
            except OSError:
                pass
            if paths:
                try:
                    name = (process / 'comm').read_text().strip()
                except OSError:
                    name = 'exited'
                holders.append({'pid': int(process.name), 'name': name, 'paths': paths})
        mounts = [line for line in Path('/proc/self/mountinfo').read_text().splitlines()
                  if prefix in line]
        self.receipt['linux_mount_diagnostic'] = {'holders': holders, 'mountinfo': mounts}

    def cell(self, name):
        self.receipt['cells'][name] = True
        self.flush()
        print(f'project vault CLI: {name}', flush=True)

    def recovery(self, name):
        return self.credentials / name

    def check_recovery(self, name):
        path = self.recovery(name)
        if os.name == 'nt':
            script = '''
$ErrorActionPreference = 'Stop'
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$acl = Get-Acl -LiteralPath $env:COZY_PROOF_RECOVERY_PATH
if (-not $acl.AreAccessRulesProtected) { throw 'recovery DACL inherits directory access' }
$allowed = @($acl.Access | Where-Object { $_.AccessControlType -eq 'Allow' })
if ($allowed.Count -eq 0) { throw 'recovery DACL has no user allow rule' }
foreach ($rule in $allowed) {
    if ($rule.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value -ne $sid) {
        throw 'recovery DACL allows another principal'
    }
}
'''
            try:
                subprocess.run(['powershell', '-NoProfile', '-NonInteractive', '-Command', script],
                               text=True, check=True,
                               env={**os.environ, 'COZY_PROOF_RECOVERY_PATH': str(path)},
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            except subprocess.CalledProcessError as error:
                raise RuntimeError('recovery output ACL check failed: '
                                   + self.redact(error.stderr[-4096:])) from None
        elif path.stat().st_mode & 0o077:
            raise RuntimeError('recovery output permits group or other-user access')
        self.redact('')

    def status(self):
        return self.command('vault', 'status')

    def view(self, changed):
        value = changed.get('vault', changed)
        if value.get('state') != 'unlocked' or not value.get('view'):
            raise RuntimeError('successful unlock did not publish a readable view')
        return Path(value['view'])

    def assert_locked(self, old_view=None, old_file=None):
        if self.status()['state'] != 'locked':
            raise RuntimeError('lock was acknowledged without reaching locked state')
        self.command('status', refusal=True)
        self.assert_inaccessible(old_view, old_file)

    def assert_inaccessible(self, old_view=None, old_file=None):
        if old_file:
            try:
                old_file.read_bytes()
            except OSError:
                pass
            else:
                raise RuntimeError('the old mounted file remained readable after locking')
        if old_view:
            try:
                (old_view / 'must-not-leak.bin').write_bytes(self.canary.encode())
            except OSError:
                pass
            else:
                raise RuntimeError('an absent mount accepted a plaintext fallback write')

    def wait_locked(self, failure='a configured two-second session deadline failed to lock'):
        # The requested two-second session deadline is the policy under test. Status polling
        # must not refresh it; the wider budget permits the actual normal unmount to finish.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if self.status()['state'] == 'locked':
                return
            time.sleep(0.25)
        raise RuntimeError(failure)

    def shutdown(self):
        # The CLI intentionally exposes no up/down verb. Exercise its real graceful restart
        # API, using only the default controller discovered by the normal CLI status verb.
        status = self.command('status', project=False)['calcifer']
        if not status.get('running'):
            return
        record = self.home / 'calcifer' / 'calcifer.lock.holder'
        holder = json.loads(record.read_text())
        if holder['pid'] != status['pid'] or holder['address'] != status['address']:
            raise RuntimeError('the default controller changed before graceful shutdown')
        base = 'http://' + holder['address']
        if urllib.parse.urlsplit(base).hostname not in ['127.0.0.1', '::1']:
            raise RuntimeError('the default controller is not loopback')
        self.secret_values.append(holder['token'])
        request = urllib.request.Request(base + '/v1/down', data=b'{}', method='POST',
                                         headers={'Authorization': 'Bearer ' + holder['token'],
                                                  'Content-Type': 'application/json'})
        print('project vault CLI lifecycle: graceful shutdown of discovered controller', flush=True)
        try:
            direct = urllib.request.build_opener(urllib.request.ProxyHandler({}))
            with direct.open(request) as response:
                json.load(response)
        except urllib.error.HTTPError as error:
            detail = error.read().decode(errors='replace')
            raise RuntimeError('graceful shutdown refused: ' + self.redact(detail)) from None
        deadline = time.monotonic() + 30
        while record.exists():
            if time.monotonic() >= deadline:
                raise RuntimeError('shutdown acknowledged but the controller holder remained published')
            time.sleep(0.05)

    def media(self):
        source = self.out / 'inputs'
        source.mkdir()
        image = source / 'image.png'
        self.ffmpeg('-f', 'lavfi', '-i', 'color=c=blue:size=8x8', '-frames:v', '1', image)
        original = image.read_bytes()
        payload = b'cozy-proof\0' + self.canary.encode()
        chunk = b'tEXt' + payload
        image.write_bytes(original[:-12] + struct.pack('>I', len(payload)) + chunk
                          + struct.pack('>I', zlib.crc32(chunk)) + original[-12:])
        video, audio = source / 'video.mp4', source / 'audio.wav'
        self.ffmpeg('-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24', '-t', '2',
                    '-c:v', 'mpeg4', '-metadata', 'comment=' + self.canary, video)
        self.ffmpeg('-f', 'lavfi', '-i', 'sine=frequency=440:duration=1',
                    '-metadata', 'comment=' + self.canary, audio)
        for path in [image, video, audio]:
            if self.canary.encode() not in path.read_bytes():
                raise RuntimeError('the real media fixture did not retain its content canary')
        return [image, video, audio]

    def ffmpeg(self, *args):
        subprocess.run([self.fixture_ffmpeg, '-v', 'error', '-nostdin', *map(str, args)], check=True)

    @staticmethod
    def pe_machine(path):
        with path.open('rb') as handle:
            if handle.read(2) != b'MZ':
                raise RuntimeError('expected a PE executable for the Windows media proof')
            handle.seek(60)
            offset = struct.unpack('<I', handle.read(4))[0]
            handle.seek(offset)
            header = handle.read(6)
        if header[:4] != b'PE\0\0':
            raise RuntimeError('invalid PE header in the Windows media proof')
        return struct.unpack('<H', header[4:])[0]

    def pinned_winarm_media(self):
        if sys.getwindowsversion().build < 22000:
            raise RuntimeError('x64 media helper emulation requires Windows 11')
        if self.pe_machine(self.binary) != 0xaa64:
            raise RuntimeError('the Cozy executable must remain native ARM64')
        deadline = time.monotonic() + FIRST_COLD_SECONDS

        def remaining():
            budget = deadline - time.monotonic()
            if budget <= 0:
                raise RuntimeError('pinned Windows ARM media preparation exceeded its first-cold budget')
            return budget

        while True:
            state = self.command('status', full=True, timeout=remaining())
            assets = {asset['name']: asset for asset in state['assets']}
            measured = {name: {key: assets.get(name, {}).get(key)
                               for key in ['width', 'height', 'duration_ms']}
                        for name in ['proof_image', 'proof_video', 'proof_audio']}
            if (measured['proof_image']['width'] == 8 and measured['proof_image']['height'] == 8
                    and measured['proof_video']['width'] == 320
                    and measured['proof_video']['height'] == 180
                    and (measured['proof_video']['duration_ms'] or 0) > 0
                    and (measured['proof_audio']['duration_ms'] or 0) > 0):
                break
            time.sleep(min(0.25, remaining()))
        helpers = []
        for name in ['ffmpeg', 'ffprobe']:
            path = self.home / 'tools' / 'ffmpeg' / '9.0.2' / f'{name}.exe'
            if not path.is_file() or self.pe_machine(path) != 0x8664:
                raise RuntimeError('Calcifer must fetch its pinned x64 media-helper pair')
            probe = subprocess.run([str(path), '-hide_banner', '-version'], capture_output=True,
                                   check=True, env=self.cli_env, timeout=remaining())
            version = probe.stdout.decode('utf-8', errors='replace').splitlines()[0]
            if not version.startswith(f'{name} version 9.0.2'):
                raise RuntimeError('the cached media helper does not report its pinned version')
            helpers.append({'path': str(path), 'architecture': 'x64 PE0x8664', 'version': version})
        self.receipt['windows_arm_media'] = {
            'cozy_architecture': 'native ARM64 PE0xaa64', 'helpers': helpers,
            'fixture_generator': self.fixture_ffmpeg, 'cozy_subprocess_system_media_hidden': True,
            'pin_cache_initially_absent': True, 'normal_cli_metadata': measured,
            'cold_budget_seconds': FIRST_COLD_SECONDS}
        self.cell('windows_arm64_pinned_media_runtime')

    @staticmethod
    def digest(path):
        digest = hashlib.sha256()
        with path.open('rb') as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b''):
                digest.update(chunk)
        return digest.hexdigest()

    @staticmethod
    def graph_export(text):
        header, separator, body = text.partition('\n')
        prefix = '# cozy-project-base: '
        if not separator or not header.startswith(prefix):
            raise RuntimeError('the actual YAML export has no project/base marker')
        owner, etag = header[len(prefix):].split(' ', 1)
        if json.loads(etag) != hashlib.sha256(body.encode()).hexdigest():
            raise RuntimeError('the actual YAML export marker does not verify its graph body')
        return owner, body

    def find_media(self, view, source):
        expected = self.digest(source)
        for path in (view / 'files').rglob('*'):
            if path.is_file() and self.digest(path) == expected:
                return path
        raise RuntimeError(f'imported {source.suffix} bytes were not stored inside the private view')

    def scan(self, root, credentials=False):
        patterns = [('project canary', self.canary.encode())]
        patterns += [('base64 project canary', base64.b64encode(self.canary.encode()))]
        if credentials:
            patterns += [('credential', value.encode()) for value in self.secret_values]
        maximum = max(len(pattern) for _, pattern in patterns)
        inspected = 0
        def scan_error(error):
            raise error

        for folder, directories, files in os.walk(root, followlinks=False, onerror=scan_error):
            folder = Path(folder)
            # Sealed empty views are deliberately unreadable; actual unlocked views are not
            # ciphertext. Exclude only the named view, keeping runtime descriptors in scope.
            directories[:] = [name for name in directories
                              if not (folder / name).is_symlink()
                              and not (name == 'view' and (self.home / 'calcifer' / 'vaults') in (folder / name).parents)]
            for name in files:
                path = folder / name
                if path.is_symlink():
                    continue
                tail = b''
                with path.open('rb') as handle:
                    for chunk in iter(lambda: handle.read(64 * 1024), b''):
                        data = tail + chunk
                        for label, pattern in patterns:
                            if pattern in data:
                                raise RuntimeError(f'{label} leaked into {path.relative_to(root)}')
                        tail = data[-maximum:]
                inspected += 1
        return inspected

    def run(self):
        self.receipt['version'] = self.command('--version', project=False, json_output=False).strip()
        initial = self.private_file('initial', secrets.token_urlsafe(32))
        changed = self.private_file('changed', secrets.token_urlsafe(32))
        rekey = self.private_file('rekey', secrets.token_urlsafe(32))
        wrong = self.private_file('wrong', secrets.token_urlsafe(32))
        plain = self.out / 'ordinary'
        self.command('project', 'new', plain, project=False)
        self.command('project', '-p', plain, 'status', project=False)
        ordinary = self.command('project', '-p', plain, 'vault', 'status', project=False)
        assert ordinary['state'] == 'plain' and not ordinary['encrypted']
        self.cell('ordinary_project_without_encryption')
        self.command('project', 'new', self.project, project=False)
        before = self.command('export', json_output=False)
        unsafe_backing = self.project / 'unsafe-recovery.txt'
        refused = self.command('encrypt', '--passphrase-file', initial, '--recovery-out', unsafe_backing,
                               '--remove-plaintext', '--idle', 0, refusal=True)
        assert json.loads(refused.stdout)['error']['code'] == 'vault_recovery_location'
        assert not unsafe_backing.exists()
        unchanged = self.status()
        assert unchanged['state'] == 'plain' and not unchanged['encrypted']
        assert before == self.command('export', json_output=False)
        self.cell('recovery_output_refuses_backing_before_conversion')
        self.command('encrypt', '--passphrase-file', initial, '--recovery-out', self.recovery('recovery1'),
                     '--remove-plaintext', '--idle', 0)
        self.check_recovery('recovery1')
        assert before == self.command('export', json_output=False)
        self.cell('convert_without_changing_logical_state')
        sources = self.media()
        for path, name in zip(sources, ['proof_image', 'proof_video', 'proof_audio']):
            self.command('import', path, name)
        if self.winarm:
            self.pinned_winarm_media()
        edit = self.out / 'state-edit.json'
        edit.write_text(json.dumps({'description': self.canary}))
        self.command('set', 'proof_image', '--input', edit)
        state = self.command('export', json_output=False)
        assert self.canary in state
        assert self.command('history')['entries']
        view = self.view(self.status())
        saved = [self.find_media(view, path) for path in sources]
        self.ffmpeg('-ss', '1', '-i', saved[1], '-frames:v', '1', '-f', 'null', '-')
        self.ffmpeg('-i', saved[2], '-f', 'null', '-')
        self.cell('encrypted_import_image_video_audio_and_crdt_edit')
        unsafe_view = view / 'unsafe-recovery.txt'
        refused = self.command('vault', 'recovery', '--recovery-out', unsafe_view, refusal=True)
        assert json.loads(refused.stdout)['error']['code'] == 'vault_recovery_location'
        assert not unsafe_view.exists()
        self.command('lock', allow_locking=True)
        self.assert_locked(view, saved[0])
        self.command('unlock', '--passphrase-file', wrong, refusal=True)
        self.view(self.command('unlock', '--recovery-file', self.recovery('recovery1'), '--idle', 0))
        assert self.command('export', json_output=False) == state
        self.cell('recovery_output_refuses_view_without_rotating_recovery')
        self.cell('lock_refusal_and_saved_recovery')
        self.command('vault', 'passphrase', '--passphrase-file', changed)
        self.command('vault', 'recovery', '--recovery-out', self.recovery('recovery2'))
        self.check_recovery('recovery2')
        self.command('lock')
        self.command('unlock', '--passphrase-file', initial, refusal=True)
        self.command('unlock', '--recovery-file', self.recovery('recovery1'), refusal=True)
        self.view(self.command('unlock', '--passphrase-file', changed, '--idle', 0))
        self.cell('replace_passphrase_and_recovery_slots')
        rotated = self.command('vault', 'rotate', '--passphrase-file', rekey,
                               '--recovery-out', self.recovery('recovery3'))
        self.check_recovery('recovery3')
        view = self.view(rotated)
        assert state == self.command('export', json_output=False)
        for path in sources:
            self.find_media(view, path)
        for forbidden in [view / 'must-refuse-recursive-export',
                          self.project / 'project.vault.data' / 'must-refuse-plaintext-export']:
            self.command('vault', 'export', '--output', forbidden, refusal=True)
            if forbidden.exists():
                raise RuntimeError('a refused recursive/backing export created its output directory')
        self.cell('plaintext_export_refuses_view_and_backing_children')
        exported = self.out / 'plaintext-export'
        self.command('vault', 'export', '--output', exported)
        for path in sources:
            self.find_media(exported, path)
        assert (exported / 'project.db').is_file()
        self.cell('full_rekey_and_explicit_plaintext_export_bytes')
        # Two normal clients lock the same unlocked project; both acknowledgements must
        # agree on the locked state. Pending-unlock ordering has separate controller coverage.
        processes = [subprocess.Popen(['cozy', '--json', 'project', '-p', str(self.project), 'lock'],
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                      env=self.cli_env)
                     for _ in range(2)]
        for process in processes:
            stdout, stderr = process.communicate()
            if process.returncode or json.loads(stdout)['vault']['state'] != 'locked':
                raise RuntimeError('parallel lock failed: ' + self.redact(stderr[-4096:]))
        self.assert_locked(view)
        self.cell('concurrent_manual_locks')
        self.command('unlock', '--passphrase-file', rekey, '--idle', 0, '--max-duration', 2)
        self.wait_locked()
        self.assert_locked()
        self.cell('absolute_expiry_despite_status_polling')
        self.command('unlock', '--recovery-file', self.recovery('recovery3'), '--idle', 2)
        self.wait_locked()
        self.assert_locked()
        self.cell('idle_expiry_without_activity')
        view = self.view(self.command('unlock', '--recovery-file', self.recovery('recovery3'), '--idle', 0))
        assert state == self.command('export', json_output=False)
        old_file = self.find_media(view, sources[0])
        # Stop only this fresh runner's normal Calcifer through the installed command. This
        # must close an unlocked project, not rely on an earlier explicit project lock.
        self.shutdown()
        self.assert_inaccessible(view, old_file)
        self.cell('normal_shutdown_closes_an_unlocked_project')
        # All retained global databases/logs are now flushed before the disk-byte scan.
        self.redact('')
        self.receipt['global_files_scanned'] = self.scan(self.home, credentials=True)
        self.receipt['encrypted_files_scanned'] = self.scan(self.project)
        self.cell('no_known_private_canary_in_global_caches_or_backing')
        # Opening the intentionally plaintext copy is an ordinary-project operation and
        # may legitimately populate ordinary caches. Do it after the encrypted-only scan.
        copied_state = self.command('project', '-p', exported, 'export', project=False, json_output=False)
        original_id, original_graph = self.graph_export(state)
        copied_id, copied_graph = self.graph_export(copied_state)
        # A plain folder copy intentionally receives a new project identity when the original
        # remains registered. Compare the entire graph body, not that anti-stale-edit marker.
        if original_graph != copied_graph:
            difference = ''.join(difflib.unified_diff(original_graph.splitlines(True), copied_graph.splitlines(True), fromfile='original graph', tofile='exported graph'))
            raise RuntimeError('plaintext export graph changed: ' + self.redact(difference[:16384]))
        if original_id == copied_id:
            raise RuntimeError('the independent folder copy reused the registered project identity')
        self.shutdown()
        self.cell('plaintext_export_opens_independently_after_lock')
        self.receipt['result'] = 'pass'
        self.flush()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--source', required=True)
    args = parser.parse_args()
    proof = Proof(args.artifacts, args.source)
    try:
        proof.run()
    except Exception as error:
        proof.receipt.update(result='failed', error=proof.redact(str(error)))
        proof.diagnose_mount_holders()
        proof.flush()
        raise
    finally:
        if proof.receipt['result'] != 'pass':
            if proof.project.exists():
                subprocess.run(['cozy', '--json', 'project', '-p', str(proof.project), 'lock'],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                               env=proof.cli_env)
            try:
                proof.shutdown()
            except Exception:
                pass
        # Credentials/recovery output are intentionally never part of published evidence.
        shutil.rmtree(proof.credentials)


if __name__ == '__main__':
    main()
