#!/usr/bin/env python3
"""Real project encryption through the default installed cozy, on an ephemeral CI host."""
import argparse
import base64
import difflib
import hashlib
from http.client import HTTPMessage
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import sqlite3
import struct
import subprocess
import sys
import time
import traceback
from typing import IO, Callable, Literal, NoReturn, Protocol, TypedDict, Unpack, cast, overload
import urllib.error
import urllib.parse
import urllib.request
import zlib

FIRST_COLD_SECONDS = 300


Arg = str | Path | int


class Holder(TypedDict):
    address: str
    token: str
    pid: int


class WindowsVersion(Protocol):
    build: int


class Asset(TypedDict, total=False):
    name: str
    field: str
    absent: bool
    path: str | None
    sha256: str | None
    width: int | None
    height: int | None
    duration_ms: int | None


class Project(TypedDict):
    id: str
    path: str
    assets: list[Asset]


class Warning(TypedDict, total=False):
    message: str


class Payload(TypedDict, total=False):
    vault: "Payload"
    state: str
    encrypted: bool
    path: str
    view: str | None
    created: bool
    id: str
    project: Project
    recovery_secret: str | None
    preserved_source: str | None
    idle_timeout_s: int
    warnings: list[Warning]
    stopping: bool
    digest: str
    length: int
    name: str
    sha256: str
    entries: list[object]
    error: dict[str, str]


class CommandBase(TypedDict, total=False):
    cwd: Path | None
    allow_locking: bool
    full: bool
    timeout: float | None


class CommandOptions(CommandBase, total=False):
    json_output: bool
    refusal: bool


class Receipt(TypedDict, total=False):
    result: str
    source: str
    os: str
    default_home: str
    installed_binary: str
    synthetic_only: bool
    cells: dict[str, bool]
    version: str
    first_manual_lock: dict[str, str | int]
    windows_arm_media: dict[str, object]
    linux_mount_diagnostic: dict[str, object]
    error: str
    media_diagnostic_error: str
    global_files_scanned: int
    encrypted_files_scanned: int
    reencrypted_files_scanned: int
    reencrypted_global_files_scanned: int
    decrypt_with_open_reader: str
    file_write_modes: dict[str, int]


class Args(argparse.Namespace):
    artifacts: Path
    source: str


class LocalOnly(urllib.request.HTTPRedirectHandler):
    def redirect_request(
        self,
        req: urllib.request.Request,
        fp: IO[bytes],
        code: int,
        msg: str,
        headers: HTTPMessage,
        newurl: str,
    ) -> None:
        # The bearer belongs to this exact controller. A redirect is a failed fixture call.
        return None


class Proof:
    def __init__(self, artifacts: Path, source: str) -> None:
        if os.environ.get('GITHUB_ACTIONS') != 'true':
            raise RuntimeError('run this default-home proof only in an ephemeral GitHub runner')
        if 'COZY_HOME' in os.environ:
            raise RuntimeError('the proof requires the normal default home')
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
        self.holder_file = self.home / 'calcifer' / 'calcifer.lock.holder'
        self.direct = urllib.request.build_opener(urllib.request.ProxyHandler({}), LocalOnly())
        self.source = source
        self.commands: list[dict[str, object]] = []
        self.canary = 'COZY_PRIVATE_PROOF_' + secrets.token_hex(24)
        self.secret_values: list[str] = []
        self.winarm = os.name == 'nt' and os.environ.get('RUNNER_ARCH') == 'ARM64'
        self.fixture_ffmpeg = 'ffmpeg'
        self.cli_env: dict[str, str] | None = None
        if self.winarm:
            self.fixture_ffmpeg = cast(str, shutil.which('ffmpeg'))
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
        self.receipt: Receipt = {'result': 'running', 'source': source, 'os': sys.platform,
                        'default_home': str(self.home), 'installed_binary': str(self.binary),
                        'synthetic_only': True, 'cells': {}}

    def private_file(self, name: str, value: str) -> Path:
        path = self.credentials / name
        with path.open('x', encoding='utf-8') as handle:
            path.chmod(0o600)
            handle.write(value + '\n')
        self.secret_values.append(value)
        return path

    def redact(self, text: str) -> str:
        for path in self.credentials.iterdir():
            if path.is_file():
                value = path.read_text().strip()
                if value and value not in self.secret_values:
                    self.secret_values.append(value)
        for value in self.secret_values:
            text = text.replace(value, '[credential redacted]')
        return text.replace(self.canary, '[synthetic canary redacted]')

    @overload
    def command(
        self,
        *args: Arg,
        json_output: bool = True,
        refusal: Literal[True],
        cwd: Path | None = None,
        allow_locking: bool = False,
        full: bool = False,
        timeout: float | None = None,
    ) -> subprocess.CompletedProcess[str]: ...
    @overload
    def command(
        self,
        *args: Arg,
        json_output: Literal[False],
        refusal: Literal[False] = False,
        cwd: Path | None = None,
        allow_locking: bool = False,
        full: bool = False,
        timeout: float | None = None,
    ) -> str: ...
    @overload
    def command(
        self,
        *args: Arg,
        json_output: Literal[True] = True,
        refusal: Literal[False] = False,
        cwd: Path | None = None,
        allow_locking: bool = False,
        full: bool = False,
        timeout: float | None = None,
    ) -> Payload: ...
    @overload
    def command(
        self,
        *args: Arg,
        json_output: bool = True,
        refusal: bool = False,
        cwd: Path | None = None,
        allow_locking: bool = False,
        full: bool = False,
        timeout: float | None = None,
    ) -> Payload | str | subprocess.CompletedProcess[str]: ...
    def command(
        self,
        *args: Arg,
        json_output: bool = True,
        refusal: bool = False,
        cwd: Path | None = None,
        allow_locking: bool = False,
        full: bool = False,
        timeout: float | None = None,
    ) -> Payload | str | subprocess.CompletedProcess[str]:
        command = ['cozy'] + (['--json'] if json_output else [])
        if full:
            command += ['--full']
        command += [str(arg) for arg in args]
        directory = self.out if cwd is None else cwd
        began = time.monotonic()
        record: dict[str, object] = {'args': command[1:], 'cwd': str(directory), 'state': 'running'}
        self.commands.append(record)
        self.flush()
        print(f'project vault CLI command: {args[0]}', flush=True)
        raw = subprocess.run(command, capture_output=True, cwd=directory,
                             env=self.cli_env, timeout=timeout)
        completed = subprocess.CompletedProcess(raw.args, raw.returncode,
                                                raw.stdout.decode('utf-8'),
                                                raw.stderr.decode('utf-8', errors='replace'))
        record.update(state='complete', exit_code=completed.returncode,
                      seconds=round(time.monotonic() - began, 3))
        if completed.returncode and json_output:
            try:
                refused_body = json.loads(completed.stdout)
                if isinstance(refused_body, dict) and isinstance(refused_body.get('error'), dict):
                    for field in ['code', 'kind']:
                        if isinstance(refused_body['error'].get(field), str):
                            record['error_' + field] = refused_body['error'][field]
            except ValueError:
                pass
        self.flush()
        if refusal:
            if completed.returncode == 0:
                raise RuntimeError(f'{args[0]} unexpectedly succeeded while access should be refused')
            return completed
        if allow_locking and completed.returncode:
            try:
                answer: Payload = json.loads(completed.stdout)
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
        result: Payload = json.loads(completed.stdout)
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

    @overload
    def vault(
        self,
        *args: Arg,
        json_output: bool = True,
        refusal: Literal[True],
        **options: Unpack[CommandBase],
    ) -> subprocess.CompletedProcess[str]: ...
    @overload
    def vault(
        self,
        *args: Arg,
        json_output: Literal[False],
        refusal: Literal[False] = False,
        **options: Unpack[CommandBase],
    ) -> str: ...
    @overload
    def vault(
        self,
        *args: Arg,
        json_output: Literal[True] = True,
        refusal: Literal[False] = False,
        **options: Unpack[CommandBase],
    ) -> Payload: ...
    def vault(self, *args: Arg, **options: Unpack[CommandOptions]) -> Payload | str | subprocess.CompletedProcess[str]:
        # Windows needs the invoking process outside the backing directory during a rename.
        return self.command('vault', '--directory', self.project, *args, **options)

    def controller(self) -> Holder:
        holder: Holder = json.loads(self.holder_file.read_text(encoding='utf-8'))
        address = holder['address']
        if not re.fullmatch(r'(?:127\.0\.0\.1|\[::1\]):[0-9]{1,5}', address):
            raise RuntimeError('the default controller must have a loopback address')
        token = holder['token']
        if not isinstance(token, str) or not token:
            raise RuntimeError('the default controller record has no bearer')
        if token not in self.secret_values:
            self.secret_values.append(token)
        return holder

    @overload
    def request(
        self,
        method: str,
        route: str,
        *,
        body: bytes | dict[str, object] | None = None,
        headers: dict[str, str] | None = None,
        json_output: Literal[False],
        holder: Holder | None = None,
    ) -> str: ...
    @overload
    def request(
        self,
        method: str,
        route: str,
        *,
        body: bytes | dict[str, object] | None = None,
        headers: dict[str, str] | None = None,
        json_output: Literal[True] = True,
        holder: Holder | None = None,
    ) -> Payload: ...
    def request(
        self,
        method: str,
        route: str,
        *,
        body: bytes | dict[str, object] | None = None,
        headers: dict[str, str] | None = None,
        json_output: bool = True,
        holder: Holder | None = None,
    ) -> Payload | str:
        if holder is None:
            holder = self.controller()
        fields = {'Authorization': 'Bearer ' + holder['token'], **(headers or {})}
        if isinstance(body, dict):
            fields['Content-Type'] = 'application/json'
            body = json.dumps(body).encode('utf-8')
        request = urllib.request.Request('http://' + holder['address'] + route, data=body,
                                         method=method, headers=fields)
        # Keep only request metadata. The bearer, edit body and returned project data stay in memory.
        record: dict[str, object] = {'method': method, 'route': route, 'state': 'running'}
        self.commands.append(record)
        self.flush()
        began = time.monotonic()
        try:
            with self.direct.open(request, timeout=30) as response:
                data = response.read()
                record.update(state='complete', http_status=response.status)
        except urllib.error.HTTPError as error:
            record.update(state='failed', http_status=error.code)
            detail = error.read(4096).decode('utf-8', errors='replace')
            raise RuntimeError(f'{method} {route} refused ({error.code}): {self.redact(detail)}') from None
        except urllib.error.URLError:
            record['state'] = 'failed'
            raise RuntimeError(f'{method} {route} could not reach the default controller') from None
        finally:
            record['seconds'] = round(time.monotonic() - began, 3)
            self.flush()
        return cast(Payload, json.loads(data)) if json_output else data.decode('utf-8')

    def project_route(self, project_id: str | None = None) -> str:
        project = self.project_id if project_id is None else project_id
        return '/v1/projects/' + urllib.parse.quote(project, safe='')

    def yaml(self, project_id: str | None = None) -> str:
        return self.request('GET', self.project_route(project_id) + '/yaml', json_output=False)

    def import_media(self, path: Path, name: str) -> None:
        data = path.read_bytes()
        digest = 'sha256:' + hashlib.sha256(data).hexdigest()
        upload = '/v1/project-files/' + urllib.parse.quote(digest, safe='')
        upload += '?' + urllib.parse.urlencode({'project': self.project_id})
        held = self.request('PUT', upload, body=data, headers={
            'Content-Type': 'application/octet-stream',
            'Content-Range': f'bytes 0-{len(data) - 1}/{len(data)}',
        })
        assert held['digest'] == digest and held['length'] == len(data)
        imported = self.request('POST', self.project_route() + '/imports',
                                body={'source': digest, 'name': name, 'exact': True},
                                headers={'Idempotency-Key': secrets.token_hex(16)})
        assert imported['name'] == name and imported['sha256'] == digest

    def flush(self) -> None:
        (self.out / 'commands.json').write_text(json.dumps(self.commands, indent=2) + '\n')
        (self.out / 'receipt.json').write_text(json.dumps(self.receipt, indent=2) + '\n')

    def diagnose_mount_holders(self) -> None:
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

    def cell(self, name: str) -> None:
        self.receipt['cells'][name] = True
        self.flush()
        print(f'project vault CLI: {name}', flush=True)

    def recovery(self, name: str) -> Path:
        return self.credentials / name

    def check_recovery(self, name: str) -> None:
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

    def status(self, timeout: float | None = None) -> Payload:
        return self.command('vault', 'status', cwd=self.project, timeout=timeout)

    def view(self, changed: Payload) -> Path:
        value = changed.get('vault', changed)
        view = value.get('view')
        if value.get('state') != 'unlocked' or not view:
            raise RuntimeError('successful unlock did not publish a readable view')
        return Path(view)

    def assert_locked(self, old_view: Path | None = None, old_file: Path | None = None) -> None:
        if self.status()['state'] != 'locked':
            raise RuntimeError('lock was acknowledged without reaching locked state')
        refused = self.command('status', cwd=self.project, refusal=True)
        assert json.loads(refused.stdout)['error']['code'] == 'project_locked'
        self.assert_inaccessible(old_view, old_file)

    def assert_inaccessible(self, old_view: Path | None = None, old_file: Path | None = None) -> None:
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

    def wait_locked(self, failure: str = 'a configured two-second session deadline failed to lock') -> None:
        # The requested two-second session deadline is the policy under test. Status polling
        # must not refresh it; the wider budget permits the actual normal unmount to finish.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if self.status()['state'] == 'locked':
                return
            time.sleep(0.25)
        raise RuntimeError(failure)

    def shutdown(self) -> None:
        # Current-directory status describes the project. Check the live controller through
        # its authenticated status API before asking that same holder to shut down.
        holder = self.controller()
        self.request('GET', '/v1/status', holder=holder)
        if self.controller() != holder:
            raise RuntimeError('the default controller changed before graceful shutdown')
        print('project vault CLI lifecycle: graceful shutdown of discovered controller', flush=True)
        if self.request('POST', '/v1/down', body={}, holder=holder).get('stopping') is not True:
            raise RuntimeError('the default controller did not acknowledge graceful shutdown')
        deadline = time.monotonic() + 30
        while self.holder_file.exists():
            if time.monotonic() >= deadline:
                raise RuntimeError('shutdown acknowledged but the controller holder remained published')
            time.sleep(0.05)

    def media(self) -> list[Path]:
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

    def ffmpeg(self, *args: Arg) -> None:
        subprocess.run([self.fixture_ffmpeg, '-v', 'error', '-nostdin', *map(str, args)], check=True)

    @staticmethod
    def pe_machine(path: Path) -> int:
        with path.open('rb') as handle:
            if handle.read(2) != b'MZ':
                raise RuntimeError('expected a PE executable for the Windows media proof')
            handle.seek(60)
            offset = struct.unpack('<I', handle.read(4))[0]
            handle.seek(offset)
            header = handle.read(6)
        if header[:4] != b'PE\0\0':
            raise RuntimeError('invalid PE header in the Windows media proof')
        return cast(int, struct.unpack('<H', header[4:])[0])

    def pinned_winarm_media(self) -> None:
        getwindowsversion = cast(Callable[[], WindowsVersion], getattr(sys, 'getwindowsversion'))
        if getwindowsversion().build < 22000:
            raise RuntimeError('x64 media helper emulation requires Windows 11')
        if self.pe_machine(self.binary) != 0xaa64:
            raise RuntimeError('the Cozy executable must remain native ARM64')
        evidence = self.receipt['windows_arm_media'] = {
            'cozy_architecture': 'native ARM64 PE0xaa64',
            'fixture_generator': self.fixture_ffmpeg, 'cozy_subprocess_system_media_hidden': True,
            'pin_cache_initially_absent': True, 'cold_budget_seconds': FIRST_COLD_SECONDS}
        deadline = time.monotonic() + FIRST_COLD_SECONDS

        def remaining() -> float:
            budget = deadline - time.monotonic()
            if budget <= 0:
                raise RuntimeError('pinned Windows ARM media preparation exceeded its first-cold budget')
            return budget

        while True:
            state = self.command('status', cwd=self.project, full=True, timeout=remaining())
            project = state['project']
            assert project['id'] == self.project_id
            assets = {asset['name']: asset for asset in project['assets']}
            measured = {name: {'width': assets.get(name, {}).get('width'),
                               'height': assets.get(name, {}).get('height'),
                               'duration_ms': assets.get(name, {}).get('duration_ms')}
                        for name in ['proof_image', 'proof_video', 'proof_audio']}
            image = assets.get('proof_image', {})
            self.winarm_image = {key: image.get(key) for key in ['path', 'sha256']}
            self.winarm_image['canonical_root'] = project.get('path')
            observed = {'normal_cli_metadata': measured,
                        'expected_assets_present': {name: name in assets for name in measured},
                        'image_row': {'field': image.get('field'), 'absent': image.get('absent', False),
                                      'path_present': bool(image.get('path'))},
                        'warnings': [self.media_diagnostic_text(warning.get('message', ''))
                                     for warning in state.get('warnings', [])[:16]]}
            if any(evidence.get(key) != value for key, value in observed.items()):
                evidence.update(observed)
                self.flush()
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
        evidence['helpers'] = helpers
        self.cell('windows_arm64_pinned_media_runtime')

    def media_diagnostic_text(self, text: object) -> str:
        text = self.redact(str(text))
        # Media errors need no long opaque tokens, including private/holder credentials.
        return re.sub(r'[A-Za-z0-9_+/-]{32,}', '[opaque value redacted]', text)[:512]

    def diagnose_winarm_media(self) -> None:
        # Failure evidence has its own short bound and can never satisfy the runtime cell.
        deadline = time.monotonic() + 10
        evidence = self.receipt.setdefault('windows_arm_media', {})
        helpers: list[dict[str, object]] = []
        diagnostic: dict[str, object] = {'budget_seconds': 10, 'helpers': helpers}
        evidence['failure_diagnostic'] = diagnostic
        try:
            with self.holder_file.open('rb') as handle:
                token = json.loads(handle.read(16384)).get('token')
            if isinstance(token, str) and token and token not in self.secret_values:
                self.secret_values.append(token)
        except (OSError, ValueError):
            pass
        cache = self.home / 'tools' / 'ffmpeg' / '9.0.2'
        for name in ['ffmpeg', 'ffprobe']:
            path = cache / f'{name}.exe'
            helper: dict[str, object] = {'path': str(path), 'present': path.is_file()}
            helpers.append(helper)
            if not helper['present']:
                continue
            try:
                helper['pe_machine'] = hex(self.pe_machine(path))
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    helper['version_probe'] = 'diagnostic budget exhausted'
                    continue
                probe = subprocess.run([str(path), '-hide_banner', '-version'],
                                       capture_output=True, env=self.cli_env, timeout=remaining)
                helper['exit_code'] = probe.returncode
                lines = (probe.stdout or probe.stderr).decode('utf-8', errors='replace').splitlines()
                helper['version'] = self.media_diagnostic_text(lines[0] if lines else '')
            except Exception as error:
                helper['error'] = self.media_diagnostic_text(error)
        try:
            self.diagnose_winarm_png(diagnostic, cache / 'ffprobe.exe', deadline)
        except Exception as error:
            diagnostic['png_diagnostic_error'] = self.media_diagnostic_text(error)
        diagnostic['partial_downloads'] = [
            {'name': path.name, 'bytes': path.stat().st_size}
            for path in sorted(cache.parent.glob('.ffmpeg-*.part'))[:4] if path.is_file()]
        log = self.home / 'calcifer' / 'calcifer.log'
        if log.is_file():
            with log.open('rb') as handle:
                handle.seek(max(0, log.stat().st_size - 65536))
                tail = handle.read(65536).decode('utf-8', errors='replace')
            markers = ['fetching ffmpeg:', "measuring a project's files:"]
            diagnostic['media_log'] = [
                self.media_diagnostic_text(line[line.index(marker):])
                for line in tail.splitlines() for marker in markers if marker in line][-12:]
        diagnostic['seconds'] = round(time.monotonic() - (deadline - 10), 3)
        self.flush()

    def diagnose_winarm_png(self, diagnostic: dict[str, object], ffprobe: Path, deadline: float) -> None:
        def remaining() -> float:
            value = deadline - time.monotonic()
            if value <= 0:
                raise RuntimeError('media failure diagnostic budget exhausted')
            return value

        def probe_file(path: Path, explicit_png: bool = False) -> dict[str, object]:
            result: dict[str, object] = {'is_file': path.is_file()}
            if not result['is_file']:
                return result
            result['bytes'] = path.stat().st_size
            # Same arguments as project/tools.rs::probe and tools.rs::Media::reading.
            formats = ('mov,matroska,avi,asf,mpeg,mpegts,flv,png_pipe,apng,jpeg_pipe,webp_pipe,'
                       'gif,bmp_pipe,tiff_pipe,jpegxl_pipe,jpegxl_anim,psd_pipe,ico,mp3,ogg,flac,'
                       'wav,aac,aiff,amr,ape,dsf')
            fields = ('stream=codec_type,codec_name,width,height,pix_fmt,r_frame_rate,nb_frames,'
                      'nb_read_packets,bit_rate,sample_rate,channels:format=duration,bit_rate,start_time')
            done = subprocess.run([str(ffprobe), '-v', 'error', '-count_packets', '-show_entries',
                                   fields, '-of', 'json', '-protocol_whitelist', 'file',
                                   '-format_whitelist', formats]
                                  + (['-f', 'png_pipe'] if explicit_png else []) + ['-i', str(path)],
                                  capture_output=True, env=self.cli_env, timeout=remaining())
            stderr = done.stderr.decode('utf-8', errors='replace').replace(str(path), '[PNG input]')
            stderr = re.sub(r'\\\\\?\\GLOBALROOT[^\r\n]*', '[NT path redacted]', stderr, flags=re.I)
            result.update(exit_code=done.returncode,
                          stderr=self.media_diagnostic_text(stderr))
            try:
                streams = json.loads(done.stdout).get('streams', [])
                result['streams'] = [{key: stream.get(key) for key in
                                      ['codec_type', 'codec_name', 'width', 'height']}
                                     for stream in streams[:4]]
            except ValueError:
                result['stdout_is_json'] = False
            return result

        original = self.out / 'inputs' / 'image.png'
        diagnostic['original_source_png'] = probe_file(original)
        selected: dict[str, str | None] = getattr(self, 'winarm_image', {})
        relative = Path(selected.get('path') or '')
        if relative.is_absolute() or relative.drive or relative.parts[:1] != ('files',) or '..' in relative.parts:
            raise RuntimeError('the image row did not select a project files path')
        status = self.status(timeout=remaining())
        view = self.view(status)
        imported = view / relative
        diagnostic['imported_png_in_unlocked_encrypted_view'] = probe_file(imported)
        canonical_root = selected.get('canonical_root') or ''
        diagnostic['canonical_input_kind'] = {
            'extended_prefix': canonical_root.startswith('\\\\?\\'),
            'globalroot_prefix': canonical_root.upper().startswith('\\\\?\\GLOBALROOT\\'),
            'contains_questionmark': '?' in canonical_root,
            'spelling_matches_vault_view': canonical_root.casefold() == str(view).casefold()}
        if not Path(canonical_root).is_absolute():
            raise RuntimeError('full status did not retain an absolute canonical project root')
        canonical = Path(canonical_root) / relative
        diagnostic['canonical_png_default'] = probe_file(canonical)
        diagnostic['canonical_png_forced_single_image'] = probe_file(canonical, explicit_png=True)
        if original.is_file() and imported.is_file() and max(original.stat().st_size, imported.stat().st_size) <= 1048576:
            diagnostic['original_imported_digest_equal'] = self.digest(original) == self.digest(imported)
        remaining()
        connection = sqlite3.connect((view / 'project.db').as_uri() + '?mode=ro', uri=True,
                                     timeout=min(0.1, remaining()))
        try:
            row = connection.execute('SELECT record FROM measured WHERE sha256 = ? LIMIT 1',
                                     (selected.get('sha256'),)).fetchone()
            record = json.loads(row[0]) if row else {}
            diagnostic['image_measured_row'] = {'present': row is not None, 'values': {
                key: record.get(key) for key in ['width', 'height', 'fps', 'frames', 'alpha', 'duration_ms']
                if isinstance(record.get(key), (int, float))}}
        finally:
            connection.close()

    @staticmethod
    def digest(path: Path) -> str:
        digest = hashlib.sha256()
        with path.open('rb') as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b''):
                digest.update(chunk)
        return digest.hexdigest()

    @staticmethod
    def graph_export(text: str) -> tuple[str, str]:
        header, separator, body = text.partition('\n')
        prefix = '# cozy-project-base: '
        if not separator or not header.startswith(prefix):
            raise RuntimeError('the actual YAML export has no project/base marker')
        owner, etag = header[len(prefix):].split(' ', 1)
        if json.loads(etag) != hashlib.sha256(body.encode()).hexdigest():
            raise RuntimeError('the actual YAML export marker does not verify its graph body')
        return owner, body

    def find_media(self, view: Path, source: Path) -> Path:
        expected = self.digest(source)
        for path in (view / 'files').rglob('*'):
            if path.is_file() and self.digest(path) == expected:
                return path
        raise RuntimeError(f'imported {source.suffix} bytes were not stored inside the private view')

    def scan(self, root: Path, credentials: bool = False) -> int:
        patterns = [('project canary', self.canary.encode())]
        patterns += [('base64 project canary', base64.b64encode(self.canary.encode()))]
        if credentials:
            patterns += [('credential', value.encode()) for value in self.secret_values]
        maximum = max(len(pattern) for _, pattern in patterns)
        inspected = 0
        def scan_error(error: OSError) -> NoReturn:
            raise error

        for folder_name, directories, files in os.walk(root, followlinks=False, onerror=scan_error):
            folder = Path(folder_name)
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

    def supervised_shutdown(self, view: Path, media: Path) -> Path:
        if os.name != 'posix':
            return view
        import selectors
        import signal
        holder = self.controller()
        pid = holder['pid']
        assert pid > 1
        digest = self.digest(media)
        log = self.home / 'calcifer/calcifer.log'
        stopped_before = log.read_text(errors='replace').count('calcifer::serve: Calcifer stopped')
        reader = subprocess.Popen([sys.executable, '-c',
                                   "import sys; held=open(sys.argv[1],'rb'); print('ready',flush=True); sys.stdin.buffer.read(1); held.close()",
                                   str(media)], cwd=view, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            assert reader.stdout is not None
            with selectors.DefaultSelector() as ready:
                ready.register(reader.stdout, selectors.EVENT_READ)
                assert ready.select(timeout=10), 'owned native reader did not become ready'
                assert reader.stdout.readline() == b'ready\n'
            try:
                self.request('POST', '/v1/down', body={}, holder=holder)
            except RuntimeError as error:
                assert 'project_busy' in str(error)
            else:
                raise AssertionError('manual down ignored a live native reader and working directory')
            activity = self.request('POST', self.project_route() + '/vault', body={'action': 'activity'}, holder=holder)
            assert activity['vault']['state'] == 'locking'
            self.cell('manual_down_busy_preserves_normal_admission')
            # Main-only TERM is systemd KillMode=mixed's normal signal. Its owned
            # provider must stay alive while this real external reader holds it.
            os.kill(pid, signal.SIGTERM)
            deadline = time.monotonic() + 30
            admission_closed = False
            while time.monotonic() < deadline:
                try:
                    self.request('POST', self.project_route() + '/vault', body={'action': 'activity'}, holder=holder)
                except RuntimeError as error:
                    if 'calcifer_stopping' not in str(error) or '(503)' not in str(error):
                        raise
                    admission_closed = True
                    break
                time.sleep(0.05)
            assert admission_closed, 'SIGTERM did not retain its shutdown admission intent after Busy'
            os.kill(pid, 0)
            assert self.digest(media) == digest
            self.cell('sigterm_waits_for_the_real_owned_mount_without_killing_its_provider')
        finally:
            if reader.stdin is not None:
                try:
                    reader.stdin.write(b'\n')
                    reader.stdin.flush()
                except BrokenPipeError:
                    pass
                reader.stdin.close()
            try:
                reader.wait(timeout=10)
            except subprocess.TimeoutExpired:
                reader.terminate()
                reader.wait(timeout=10)
        assert reader.returncode == 0, 'owned native reader did not exit normally'
        deadline = time.monotonic() + 30
        while True:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            if time.monotonic() >= deadline:
                raise AssertionError('Calcifer did not exit after its native reader released the view')
            time.sleep(0.05)
        assert log.read_text(errors='replace').count('calcifer::serve: Calcifer stopped') > stopped_before
        self.assert_inaccessible(view, media)
        assert self.status()['state'] == 'locked'
        reopened = self.view(self.vault('unlock', '--recovery-file', self.recovery('recovery1'), '--idle', 0))
        assert self.digest(reopened / media.relative_to(view)) == digest
        self.cell('sigterm_drains_then_exits_gracefully_and_reopens_unchanged_media')
        return reopened

    def preservation_policy(self, password: Path) -> None:
        directory = self.out / 'explicit-preservation'
        made = self.command('new', '--unencrypted', directory)
        payload = directory / 'retained-source.txt'
        payload.write_bytes(b'Synthetic verified preservation payload')
        digest = self.digest(payload)
        refused = self.command('vault', '--directory', directory, 'encrypt', '--passphrase-file', password,
                               '--preserve-plaintext', '--remove-plaintext', refusal=True)
        assert refused.returncode != 0 and payload.is_file()
        status = self.command('vault', '--directory', directory, 'status')
        assert status['state'] == 'plain' and not status['encrypted']
        self.cell('conflicting_source_flags_refuse_before_conversion')
        try:
            self.request('POST', '/v1/projects/' + made['id'] + '/vault',
                         body={'action': 'encrypt', 'passphrase': password.read_text().strip(),
                               'remove_plaintext': True, 'preserve_plaintext': True})
        except RuntimeError as error:
            assert 'vault_source_policy' in str(error)
        else:
            raise AssertionError('conflicting API source policies changed storage')
        assert self.digest(payload) == digest and not (directory / 'project.vault.keys.json').exists()
        self.cell('conflicting_api_source_policy_keeps_plaintext')
        if os.name != 'nt':
            outside = self.out / 'unrelated-source.txt'
            outside.write_bytes(b'Synthetic unrelated file')
            link = directory / 'external-source.txt'
            link.symlink_to(outside)
            try:
                failed = self.command('vault', '--directory', directory, 'encrypt', '--passphrase-file', password,
                                      '--recovery-out', self.recovery('refused-source'), refusal=True)
                assert json.loads(failed.stdout)['error']['code'] == 'vault_symlink'
                assert self.digest(payload) == digest and outside.read_bytes() == b'Synthetic unrelated file'
                assert not (directory / 'project.vault.keys.json').exists()
                assert not self.recovery('refused-source').exists()
                self.cell('failed_conversion_keeps_the_good_plaintext_source')
            finally:
                link.unlink()
                outside.unlink()
        before = set(self.out.glob('.cozy-project-source-*'))
        kept = self.command('vault', '--directory', directory, 'encrypt', '--passphrase-file', password,
                            '--recovery-out', self.recovery('preserved-source'), '--preserve-plaintext', '--idle', 0)
        self.check_recovery('preserved-source')
        source = kept.get('preserved_source')
        assert source
        preserved = Path(source)
        added = set(self.out.glob('.cozy-project-source-*')) - before
        assert len(added) == 1 and preserved.samefile(next(iter(added)))
        assert self.digest(preserved / payload.name) == digest
        assert self.digest(self.view(kept) / payload.name) == digest
        assert not payload.exists()
        self.command('vault', '--directory', directory, 'lock')
        self.shutdown()
        status = self.command('vault', '--directory', directory, 'status')
        assert status['encrypted'] and status['state'] == 'locked'
        assert self.digest(preserved / payload.name) == digest
        self.cell('explicit_preservation_keeps_and_reports_the_original_after_restart')
        shutil.rmtree(preserved)

    def open_in_tty(self, password: str | None, environment: dict[str, str], *options: str) -> tuple[int, str, str]:
        # Real console input, not a credential mock. Raw console/session-code output stays in memory.
        import errno
        import fcntl
        import pty
        import select
        import termios

        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 100, 0, 0))
        def controlling_terminal() -> None:
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        arguments = ['cozy', *options, 'open']
        if '--port' in options:
            arguments = ['cozy', 'open', *options]
        record: dict[str, object] = {'args': arguments[1:], 'cwd': str(self.project), 'state': 'running', 'pty': True}
        self.commands.append(record)
        self.flush()
        process = subprocess.Popen(arguments, cwd=self.project, env=environment, stdin=slave,
                                   stdout=subprocess.PIPE, stderr=slave, text=True, encoding='utf-8',
                                   preexec_fn=controlling_terminal)
        os.close(slave)
        transcript = bytearray()
        entered = False
        prompt = b'Project passphrase: '
        deadline = time.monotonic() + FIRST_COLD_SECONDS
        try:
            while process.poll() is None:
                if time.monotonic() >= deadline:
                    raise RuntimeError('interactive open did not finish within its native CLI deadline')
                if select.select([master], [], [], 0.05)[0]:
                    try:
                        block = os.read(master, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        block = b''
                    transcript.extend(block)
                if transcript.count(prompt) > 1:
                    raise RuntimeError('open repeated its credential prompt instead of returning the failure')
                if prompt in transcript and not entered:
                    if password is None:
                        raise RuntimeError('JSON open unexpectedly requested a terminal credential')
                    # Wait for the actual console echo guard, not a timing sleep, before typing.
                    if not termios.tcgetattr(master)[3] & termios.ECHO:
                        os.write(master, password.encode('utf-8') + b'\n')
                        entered = True
            stdout, _ = process.communicate(timeout=FIRST_COLD_SECONDS)
            while select.select([master], [], [], 0)[0]:
                try:
                    block = os.read(master, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        break
                    raise
                if not block:
                    break
                transcript.extend(block)
            stderr = transcript.decode('utf-8', errors='replace')
            if password is not None:
                assert entered, 'interactive open did not request a hidden credential'
                assert password not in stdout and password not in stderr, 'interactive open echoed its credential'
            assert process.returncode is not None
            record.update(state='complete', exit_code=process.returncode, hidden_input=entered)
            return process.returncode, stdout, stderr
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate()
            os.close(master)
            self.flush()

    def open_unlocks_locked_project(self, password: Path, wrong: Path) -> None:
        refused = self.command('open', cwd=self.project, refusal=True)
        assert json.loads(refused.stdout)['error']['code'] == 'project_locked'
        assert 'Project passphrase:' not in refused.stdout + refused.stderr
        plain = self.command('open', cwd=self.project, refusal=True, json_output=False)
        assert 'cozy vault unlock' in plain.stdout + plain.stderr
        assert 'Project passphrase:' not in plain.stdout + plain.stderr
        self.assert_locked()
        self.cell('open_noninteractive_and_json_keep_unlock_guidance')
        if os.name != 'posix':
            return  # POSIX PTY below; Windows still qualifies the noninteractive guard above.
        import socket

        browser_bin = self.out / 'open-browser-bin'
        browser_bin.mkdir()
        browser_log = self.out / 'open-browser-argv.txt'
        browser = browser_bin / ('open' if sys.platform == 'darwin' else 'xdg-open')
        browser.write_text('#!/bin/sh\nprintf \'%s\\n\' "$1" >> "$COZY_TEST_BROWSER_LOG"\n')
        browser.chmod(0o755)
        environment = dict(os.environ if self.cli_env is None else self.cli_env)
        environment['PATH'] = str(browser_bin) + os.pathsep + environment.get('PATH', '')
        environment['COZY_TEST_BROWSER_LOG'] = str(browser_log)
        code, stdout, stderr = self.open_in_tty(None, environment, '--json')
        assert code != 0 and json.loads(stdout)['error']['code'] == 'project_locked'
        assert 'Project passphrase:' not in stderr and not browser_log.exists()
        self.cell('open_json_with_a_real_tty_still_never_prompts')
        code, stdout, _ = self.open_in_tty(wrong.read_text().strip(), environment)
        assert code != 0 and not browser_log.exists()
        self.assert_locked()
        self.cell('open_wrong_hidden_credential_stays_locked_without_a_browser')
        code, stdout, _ = self.open_in_tty(password.read_text().strip(), environment)
        assert code == 0 and 'project:' in stdout and 'url:' in stdout
        assert 'readable view:' not in stdout and ': unlocked' not in stdout
        assert self.status()['state'] == 'unlocked' and browser_log.exists()
        self.cell('open_unlocks_once_and_keeps_only_the_normal_open_response')
        self.vault('lock', allow_locking=True)
        self.assert_locked()
        with socket.socket() as available:
            available.bind(('127.0.0.1', 0))
            port = available.getsockname()[1]
        code, stdout, _ = self.open_in_tty(password.read_text().strip(), environment, '--port', str(port))
        assert code == 0 and 'url:' in stdout
        assert self.controller()['address'].endswith(':' + str(port)) and self.status()['state'] == 'unlocked'
        assert len(browser_log.read_text().splitlines()) == 2
        self.cell('open_port_unlocks_the_selected_daemon_without_rollback')
        self.vault('lock', allow_locking=True)
        self.assert_locked()

    def creation_lock_race(self, password: Path) -> None:
        directory = self.out / 'creation-lock-race'
        args = ['cozy', '--json', 'new', str(directory), '--passphrase-file', str(password),
                '--recovery-out', str(self.recovery('creation-race'))]
        record: dict[str, object] = {'args': args[1:], 'cwd': str(self.out), 'state': 'running'}
        self.commands.append(record)
        self.flush()
        creation = subprocess.Popen(args, cwd=self.out, env=self.cli_env, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, text=True, encoding='utf-8')
        try:
            deadline = time.monotonic() + FIRST_COLD_SECONDS
            while not (directory / 'project.vault.keys.json').exists() and creation.poll() is None:
                if time.monotonic() >= deadline:
                    raise RuntimeError('new project did not reach its creation barrier')
                time.sleep(0.01)
            assert (directory / 'project.vault.keys.json').exists(), 'creation did not publish its wrapped key'
            assert creation.poll() is None, 'creation finished before the concurrent lock was exercised'
            lock_args = ['cozy', '--json', 'vault', '--directory', str(directory), 'lock']
            locked = subprocess.run(lock_args, cwd=self.out, env=self.cli_env, capture_output=True,
                                    text=True, encoding='utf-8', timeout=FIRST_COLD_SECONDS)
            self.commands.append({'args': lock_args[1:], 'cwd': str(self.out), 'state': 'complete', 'exit_code': locked.returncode})
            output, stderr = creation.communicate(timeout=FIRST_COLD_SECONDS)
            record.update(state='complete', exit_code=creation.returncode)
            if locked.returncode:
                assert json.loads(locked.stdout)['error']['code'] == 'project_busy'
            if creation.returncode:
                assert json.loads(output)['error']['code'] in ('project_locked', 'project_busy'), self.redact(stderr)
            state = self.command('vault', '--directory', directory, 'status')
            deadline = time.monotonic() + FIRST_COLD_SECONDS
            while state['state'] == 'locking' and time.monotonic() < deadline:
                time.sleep(0.05)
                state = self.command('vault', '--directory', directory, 'status')
            assert state['state'] == 'locked' and not state.get('view'), 'creation reopened after the concurrent lock'
            self.cell('new_project_cannot_reopen_after_lock')
        finally:
            if creation.poll() is None:
                creation.kill()
                creation.communicate()
            self.flush()

    def run(self) -> None:
        self.receipt['version'] = self.command('--version', json_output=False).strip()
        self.command('vault', 'export', '--help', json_output=False, refusal=True)
        self.cell('removed_vault_export_command')
        initial = self.private_file('initial', secrets.token_urlsafe(32))
        changed = self.private_file('changed', secrets.token_urlsafe(32))
        rekey = self.private_file('rekey', secrets.token_urlsafe(32))
        wrong = self.private_file('wrong', secrets.token_urlsafe(32))
        plain = self.out / 'ordinary'
        created = self.command('new', '--unencrypted', plain)
        assert created['created']
        plain_status = self.command('status', cwd=plain, full=True)
        assert plain_status['project']['id'] == created['id']
        ordinary = self.command('vault', 'status', cwd=plain)
        assert ordinary['state'] == 'plain' and not ordinary['encrypted']
        self.cell('ordinary_project_without_encryption')

        missing = self.out / 'missing-creation-credential'
        refused = self.command('new', missing, refusal=True)
        assert json.loads(refused.stdout)['error']['code'] == 'vault_credential'
        assert not missing.exists()
        try:
            self.request('POST', '/v1/projects', body={'path': str(missing)})
        except RuntimeError as error:
            assert 'vault_credential' in str(error)
        else:
            raise AssertionError('API creation silently opted out of encryption')
        assert not missing.exists()
        self.cell('new_project_requires_a_known_credential')

        try:
            self.request('POST', '/v1/projects', body={'path': str(missing), 'passphrase': initial.read_text()},
                         headers={'Idempotency-Key': 'must-not-cache-creation-recovery'})
        except RuntimeError as error:
            assert 'vault_idempotency' in str(error)
        else:
            raise AssertionError('encrypted creation accepted a globally cached response')
        assert not missing.exists()
        occupied = self.out / 'nonempty-new-project'
        occupied.mkdir()
        unrelated = occupied / 'keep.txt'
        unrelated.write_bytes(b'Existing unrelated data must be preserved.\n')
        refused = self.command('new', occupied, '--passphrase-file', initial, refusal=True)
        assert json.loads(refused.stdout)['error']['code'] == 'project_not_empty'
        assert unrelated.read_bytes() == b'Existing unrelated data must be preserved.\n'
        assert sorted(path.name for path in occupied.iterdir()) == ['keep.txt']
        self.cell('new_encryption_refuses_caching_and_unrelated_plaintext')

        fresh = self.out / 'default-encrypted'
        preserved_before = set(self.out.glob('.cozy-project-source-*'))
        new = self.command('new', fresh, '--passphrase-file', initial, '--recovery-out', self.recovery('new-project'))
        assert new['created'] and 'recovery_secret' not in new
        self.check_recovery('new-project')
        protected = self.command('vault', '--directory', fresh, 'status')
        assert protected['encrypted'] and protected['state'] == 'unlocked'
        assert protected['idle_timeout_s'] == 1800
        created_view = protected['view']
        assert created_view is not None
        assert (Path(created_view) / 'project.db').is_file()
        assert not (fresh / 'project.db').exists() and not (fresh / 'project.db.new').exists()
        assert set(self.out.glob('.cozy-project-source-*')) == preserved_before
        existing = self.command('new', fresh)
        assert not existing['created'] and existing['id'] == new['id']
        self.cell('new_project_starts_in_encrypted_storage')
        self.command('vault', '--directory', fresh, 'lock')
        default_unlock = self.command('vault', '--directory', fresh, 'unlock', '--recovery-file', self.recovery('new-project'))
        assert default_unlock['vault']['idle_timeout_s'] == 1800
        created_reopened = self.command('status', cwd=fresh, full=True)
        assert created_reopened['project']['id'] == new['id']
        self.command('vault', '--directory', fresh, 'lock')
        self.cell('new_project_recovery_reopens_same_identity')
        self.creation_lock_race(initial)

        created = self.command('new', '--unencrypted', self.project)
        assert created['created']
        # The compact code is for display. Project routes and upload scope use the canonical ID.
        self.project_id = created['id']
        assert self.command('status', cwd=self.project, full=True)['project']['id'] == self.project_id
        before = self.yaml()
        unsafe_backing = self.project / 'unsafe-recovery.txt'
        refused = self.vault('encrypt', '--passphrase-file', initial, '--recovery-out', unsafe_backing,
                             '--remove-plaintext', '--idle', 0, refusal=True)
        assert json.loads(refused.stdout)['error']['code'] == 'vault_recovery_location'
        assert not unsafe_backing.exists()
        unchanged = self.status()
        assert unchanged['state'] == 'plain' and not unchanged['encrypted']
        assert unchanged['idle_timeout_s'] == 1800
        assert before == self.yaml()
        self.cell('recovery_output_refuses_backing_before_conversion')
        self.project.joinpath('files').mkdir(exist_ok=True)
        stream_fixture = Path(__file__).resolve().parents[1] / 'cli' / 'vault_stream.py'
        streamed = subprocess.run([sys.executable, str(stream_fixture), str(self.project), str(initial),
                                   str(self.recovery('recovery1'))], capture_output=True, text=True, timeout=300)
        if streamed.returncode:
            raise AssertionError(self.redact(streamed.stdout + streamed.stderr))
        self.cell('vault_acknowledgment_reattach_and_missing_media')
        assert self.status()['idle_timeout_s'] == 0
        self.check_recovery('recovery1')
        assert before == self.yaml()
        self.cell('convert_without_changing_logical_state')
        sources = self.media()
        for path, name in zip(sources, ['proof_image', 'proof_video', 'proof_audio']):
            self.import_media(path, name)
        if self.winarm:
            self.pinned_winarm_media()
        self.request('PATCH', self.project_route() + '/assets/proof_image',
                     body={'fields': {'description': self.canary}})
        state = self.yaml()
        assert self.canary in state
        assert self.request('GET', self.project_route() + '/history')['entries']
        view = self.view(self.status())
        saved = [self.find_media(view, path) for path in sources]
        self.ffmpeg('-ss', '1', '-i', saved[1], '-frames:v', '1', '-f', 'null', '-')
        self.ffmpeg('-i', saved[2], '-f', 'null', '-')
        self.cell('encrypted_import_image_video_audio_and_crdt_edit')
        unsafe_view = view / 'unsafe-recovery.txt'
        refused = self.vault('recovery', '--recovery-out', unsafe_view, refusal=True)
        assert json.loads(refused.stdout)['error']['code'] == 'vault_recovery_location'
        assert not unsafe_view.exists()
        if os.name == 'posix':
            view = self.supervised_shutdown(view, saved[1])
            saved = [self.find_media(view, path) for path in sources]
        self.vault('lock', allow_locking=True)
        self.assert_locked(view, saved[0])
        self.open_unlocks_locked_project(initial, wrong)
        self.vault('unlock', '--passphrase-file', wrong, refusal=True)
        self.view(self.vault('unlock', '--recovery-file', self.recovery('recovery1'), '--idle', 0))
        assert self.yaml() == state
        self.cell('recovery_output_refuses_view_without_rotating_recovery')
        self.cell('lock_refusal_and_saved_recovery')
        self.vault('passphrase', '--passphrase-file', changed)
        self.vault('recovery', '--recovery-out', self.recovery('recovery2'))
        self.check_recovery('recovery2')
        self.vault('lock')
        self.vault('unlock', '--passphrase-file', initial, refusal=True)
        self.vault('unlock', '--recovery-file', self.recovery('recovery1'), refusal=True)
        self.view(self.vault('unlock', '--passphrase-file', changed, '--idle', 0))
        self.cell('replace_passphrase_and_recovery_slots')
        rotated = self.vault('rotate', '--passphrase-file', rekey,
                             '--recovery-out', self.recovery('recovery3'))
        self.check_recovery('recovery3')
        view = self.view(rotated)
        assert state == self.yaml()
        for path in sources:
            self.find_media(view, path)
        self.cell('full_rekey_preserves_media_bytes')
        # Two normal clients lock the same unlocked project; both acknowledgements must
        # agree on the locked state. Pending-unlock ordering has separate controller coverage.
        processes = [subprocess.Popen(['cozy', '--json', 'vault', '--directory', str(self.project), 'lock'],
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                      cwd=self.out, env=self.cli_env)
                     for _ in range(2)]
        for process in processes:
            stdout, stderr = process.communicate()
            if process.returncode or json.loads(stdout)['vault']['state'] != 'locked':
                raise RuntimeError('parallel lock failed: ' + self.redact(stderr[-4096:]))
        self.assert_locked(view)
        self.cell('concurrent_manual_locks')
        self.vault('unlock', '--passphrase-file', rekey, '--idle', 0, '--max-duration', 2)
        self.wait_locked()
        self.assert_locked()
        self.cell('absolute_expiry_despite_status_polling')
        self.vault('unlock', '--recovery-file', self.recovery('recovery3'), '--idle', 2)
        self.wait_locked()
        self.assert_locked()
        self.cell('idle_expiry_without_activity')
        view = self.view(self.vault('unlock', '--recovery-file', self.recovery('recovery3'), '--idle', 0))
        assert state == self.yaml()
        old_file = self.find_media(view, sources[0])
        # The normal graceful shutdown must close the still-unlocked project.
        self.shutdown()
        self.assert_inaccessible(view, old_file)
        self.cell('normal_shutdown_closes_an_unlocked_project')
        # All retained global databases/logs are now flushed before the disk-byte scan.
        self.redact('')
        self.receipt['global_files_scanned'] = self.scan(self.home, credentials=True)
        self.receipt['encrypted_files_scanned'] = self.scan(self.project)
        self.cell('no_known_private_canary_in_global_caches_or_backing')
        refused = self.vault('decrypt', refusal=True)
        assert json.loads(refused.stdout)['error']['code'] == 'project_locked'
        self.cell('decrypt_refuses_locked_project')
        view = self.view(self.vault('unlock', '--recovery-file', self.recovery('recovery3'), '--idle', 0))
        readonly = view / 'readonly-retained.txt'
        readonly.write_text(self.canary)
        readonly.chmod(0o444)
        # Securefs 2.0 on Windows acknowledges chmod without changing attributes.
        # Preserve what the mounted source actually exposes through both conversions.
        write_mode = readonly.stat().st_mode & 0o222
        if os.name != 'nt':
            assert write_mode == 0
        file_modes = self.receipt['file_write_modes'] = {'source': write_mode}
        held_media = self.find_media(view, sources[0])
        decrypted: Payload | None = None
        with held_media.open('rb'):
            attempted = subprocess.run(
                ['cozy', '--json', 'vault', '--directory', str(self.project), 'decrypt'],
                capture_output=True, cwd=self.out, env=self.cli_env,
            )
            answer: Payload = json.loads(attempted.stdout)
            if attempted.returncode == 0:
                decrypted = answer['vault']
                assert decrypted['state'] == 'plain' and not decrypted['encrypted']
                self.receipt['decrypt_with_open_reader'] = 'completed_native_unmount'
            else:
                assert answer.get('error', {}).get('code') == 'project_busy'
                assert self.status()['encrypted'] and (self.project / 'project.vault.keys.json').is_file()
                self.receipt['decrypt_with_open_reader'] = 'busy_source_preserved'
        if decrypted is None:
            view = self.view(self.vault('unlock', '--recovery-file', self.recovery('recovery3'), '--idle', 0))
            decrypted = self.vault('decrypt')['vault']
        self.cell('decrypt_open_reader_has_a_safe_outcome')
        assert decrypted['state'] == 'plain' and not decrypted['encrypted']
        assert not (self.project / 'project.vault.keys.json').exists()
        assert not (self.project / 'project.vault.data').exists()
        self.assert_inaccessible(view)
        for path in sources:
            self.find_media(self.project, path)
        assert (self.project / readonly.name).read_text() == self.canary
        file_modes['decrypted'] = (self.project / readonly.name).stat().st_mode & 0o222
        assert file_modes['decrypted'] == write_mode
        assert self.yaml() == state
        assert self.command('status', cwd=self.project, full=True)['project']['id'] == self.project_id
        assert self.vault('decrypt')['vault']['state'] == 'plain'
        self.cell('decrypt_in_place_preserves_identity_graph_and_media')
        self.shutdown()
        copied_path = self.out / 'plain-folder-copy'
        shutil.copytree(self.project, copied_path)
        reopened = self.command('status', cwd=self.project, full=True)['project']
        assert reopened['id'] == self.project_id and self.yaml() == state
        self.cell('decrypted_project_reopens_after_controller_restart')
        copied = self.command('status', cwd=copied_path, full=True)['project']
        copied_state = self.yaml(copied['id'])
        original_id, original_graph = self.graph_export(state)
        copied_id, copied_graph = self.graph_export(copied_state)
        assert original_id == self.project_id and copied_id == copied['id']
        # A plain folder copy intentionally receives a new project identity when the original
        # remains registered. Compare the entire graph body, not that anti-stale-edit marker.
        if original_graph != copied_graph:
            difference = ''.join(difflib.unified_diff(original_graph.splitlines(True), copied_graph.splitlines(True), fromfile='original graph', tofile='copied graph'))
            raise RuntimeError('plaintext folder copy graph changed: ' + self.redact(difference[:16384]))
        if original_id == copied_id:
            raise RuntimeError('the independent folder copy reused the registered project identity')
        self.cell('decrypted_folder_copy_opens_with_its_own_identity')
        sources_before = set(self.out.glob('.cozy-project-source-*'))
        copied_encrypted = self.request('POST', '/v1/projects/' + copied['id'] + '/vault',
                                       body={'action': 'encrypt', 'passphrase': rekey.read_text().strip(),
                                             'remove_plaintext': False, 'idle_timeout_s': 0})
        copied_encrypted.pop('recovery_secret', None)
        assert not copied_encrypted.get('preserved_source')
        assert set(self.out.glob('.cozy-project-source-*')) == sources_before
        self.cell('legacy_false_removes_verified_plaintext')
        copied_view = self.view(copied_encrypted)
        assert self.command('status', cwd=copied_path, full=True)['project']['id'] == copied['id']
        assert self.yaml(copied['id']) == copied_state
        marker = json.loads((copied_view / '.vault-local-execution.json').read_text())
        assert marker['storage']['project_id'] == copied['id']
        assert marker['storage']['project_id'] != self.project_id
        for path in sources:
            self.find_media(copied_view, path)
        self.command('vault', '--directory', copied_path, 'lock')
        self.cell('decrypted_folder_copy_encrypts_with_its_own_namespace')
        reencrypted = self.vault('encrypt', '--passphrase-file', rekey,
                                 '--recovery-out', self.recovery('recovery4'))
        assert reencrypted['vault']['idle_timeout_s'] == 1800
        self.cell('default_idle_is_thirty_minutes_and_explicit_zero_is_preserved')
        assert not reencrypted.get('preserved_source')
        assert set(self.out.glob('.cozy-project-source-*')) == sources_before
        self.check_recovery('recovery4')
        view = self.view(reencrypted)
        assert self.yaml() == state
        assert self.command('status', cwd=self.project, full=True)['project']['id'] == self.project_id
        for path in sources:
            self.find_media(view, path)
        assert (view / readonly.name).read_text() == self.canary
        file_modes['reencrypted'] = (view / readonly.name).stat().st_mode & 0o222
        assert file_modes['reencrypted'] == write_mode
        self.cell('file_permissions_survive_decrypt_and_reencrypt')
        self.vault('lock', allow_locking=True)
        self.preservation_policy(rekey)
        self.shutdown()
        self.receipt['reencrypted_files_scanned'] = self.scan(self.project)
        self.receipt['reencrypted_global_files_scanned'] = self.scan(self.home, credentials=True)
        self.cell('reencrypt_decrypted_project_and_restore_locked_storage')
        self.receipt['result'] = 'pass'
        self.flush()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--source', required=True)
    args = parser.parse_args(namespace=Args())
    proof = Proof(args.artifacts, args.source)
    try:
        proof.run()
    except Exception as error:
        failed = traceback.extract_tb(error.__traceback__)[-1]
        detail = proof.redact(f'{type(error).__name__} in {failed.name}:{failed.lineno}: {error}')
        proof.receipt.update({'result': 'failed', 'error': detail})
        if proof.winarm:
            try:
                proof.diagnose_winarm_media()
            except Exception as diagnostic_error:
                proof.receipt['media_diagnostic_error'] = type(diagnostic_error).__name__
        proof.diagnose_mount_holders()
        proof.flush()
        raise SystemExit(detail) from None
    finally:
        try:
            if proof.receipt['result'] != 'pass':
                if proof.project.exists():
                    subprocess.run(['cozy', '--json', 'vault', '--directory', str(proof.project), 'lock'],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                   cwd=proof.out, env=proof.cli_env)
                try:
                    proof.shutdown()
                except Exception:
                    pass
        finally:
            shutil.rmtree(proof.credentials)


if __name__ == '__main__':
    main()
