#!/usr/bin/env python3
"""Real project encryption through the default installed cozy, on an ephemeral CI host."""
import argparse
import base64
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
import zlib


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

    def command(self, *args, json_output=True, refusal=False, project=True):
        command = ['cozy'] + (['--json'] if json_output else [])
        if project:
            command += ['project', '-p', str(self.project)]
        command += [str(arg) for arg in args]
        began = time.monotonic()
        record = {'args': command[1:], 'state': 'running'}
        self.commands.append(record)
        self.flush()
        print(f'project vault CLI command: {args[0]}', flush=True)
        completed = subprocess.run(command, capture_output=True, text=True)
        record.update(state='complete', exit_code=completed.returncode,
                      seconds=round(time.monotonic() - began, 3))
        self.flush()
        if refusal:
            if completed.returncode == 0:
                raise RuntimeError(f'{args[0]} unexpectedly succeeded while access should be refused')
            return completed
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
        return result

    def flush(self):
        (self.out / 'commands.json').write_text(json.dumps(self.commands, indent=2) + '\n')
        (self.out / 'receipt.json').write_text(json.dumps(self.receipt, indent=2) + '\n')

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

    def wait_locked(self):
        # The requested two-second session deadline is the policy under test. Status polling
        # must not refresh it; the wider budget permits the actual normal unmount to finish.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if self.status()['state'] == 'locked':
                return
            time.sleep(0.25)
        raise RuntimeError('a configured two-second session deadline failed to lock')

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

    @staticmethod
    def ffmpeg(*args):
        subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', *map(str, args)], check=True)

    @staticmethod
    def digest(path):
        digest = hashlib.sha256()
        with path.open('rb') as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b''):
                digest.update(chunk)
        return digest.hexdigest()

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
        self.command('encrypt', '--passphrase-file', initial, '--recovery-out', self.recovery('recovery1'),
                     '--remove-plaintext', '--idle', 0)
        self.check_recovery('recovery1')
        assert before == self.command('export', json_output=False)
        self.cell('convert_without_changing_logical_state')
        sources = self.media()
        for path, name in zip(sources, ['proof_image', 'proof_video', 'proof_audio']):
            self.command('import', path, name)
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
        self.command('lock')
        self.assert_locked(view, saved[0])
        self.command('unlock', '--passphrase-file', wrong, refusal=True)
        self.view(self.command('unlock', '--recovery-file', self.recovery('recovery1'), '--idle', 0))
        assert self.command('export', json_output=False) == state
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
        exported = self.out / 'plaintext-export'
        self.command('vault', 'export', '--output', exported)
        for path in sources:
            self.find_media(exported, path)
        assert (exported / 'project.db').is_file()
        self.cell('full_rekey_and_explicit_plaintext_export_bytes')
        # Two normal clients lock the same unlocked project; both acknowledgements must
        # agree on the locked state. Pending-unlock ordering has separate controller coverage.
        processes = [subprocess.Popen(['cozy', '--json', 'project', '-p', str(self.project), 'lock'],
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
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
        self.command('down', project=False, json_output=False)
        self.assert_inaccessible(view, old_file)
        self.cell('normal_shutdown_closes_an_unlocked_project')
        # All retained global databases/logs are now flushed before the disk-byte scan.
        self.redact('')
        self.receipt['global_files_scanned'] = self.scan(self.home, credentials=True)
        self.receipt['encrypted_files_scanned'] = self.scan(self.project)
        self.cell('no_known_private_canary_in_global_caches_or_backing')
        # Opening the intentionally plaintext copy is an ordinary-project operation and
        # may legitimately populate ordinary caches. Do it after the encrypted-only scan.
        assert state == self.command('project', '-p', exported, 'export', project=False, json_output=False)
        self.command('down', project=False, json_output=False)
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
        proof.flush()
        raise
    finally:
        if proof.receipt['result'] != 'pass':
            if proof.project.exists():
                subprocess.run(['cozy', '--json', 'project', '-p', str(proof.project), 'lock'],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run(['cozy', 'down'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        # Credentials/recovery output are intentionally never part of published evidence.
        shutil.rmtree(proof.credentials)


if __name__ == '__main__':
    main()
