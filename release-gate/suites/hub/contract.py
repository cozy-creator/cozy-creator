"""The Hub's API to an anonymous caller (tensorhub api/openapi.yaml, as the Hub serves it at GET /openapi.yaml): a
path open to anyone answers 200, a bearer-only path refuses with 401. cells.sh sends these calls through e2e/hubcheck,
which holds each request and response to that document. This is the one place in release-gate that speaks HTTP to the
Hub by hand: it is the contract test, for the paths no cozy verb reaches as a person uses it (cells.sh drives the rest
through `cozy`). Prints `check<TAB>PASS|FAIL<TAB>what` rows for expect.
    contract.py <hub> paths
    contract.py <hub> paths-empty                             a fresh Hub (tensorhub's contract.yaml): a path that names a
                                                              package, model, SKU or distribution may answer 404, since
                                                              an empty Hub has none; hubcheck still holds its body
    contract.py <hub> yanked <org> <distribution> <version>   the simple index marks it data-yanked"""
import re
import sys
import urllib.error
import urllib.request
from typing import TypedDict

import yaml

HUB, MODE = sys.argv[1].rstrip('/'), sys.argv[2]
# The first-party root namespace and a package, model and SKU every Hub release serves.
PATHS: list[tuple[str, dict[str, str]]] = [('/v1/software', {}), ('/.well-known/oauth-protected-resource', {}),
         ('/v1/auth/.well-known/jwks.json', {}), ('/v1/packages', {}), ('/v1/packages/{org}/{name}', {'org': '_', 'name': 'sdxl'}),
         ('/v1/models', {}), ('/v1/models/{org}/{name}', {'org': 'stabilityai', 'name': 'sdxl'}), ('/v1/rental-skus', {}),
         ('/v1/rental-skus/{name}', {'name': 'cpu'}), ('/v1/execution-environment', {}), ('/v1/index/{org}/simple/', {'org': '_'}),
         ('/v1/index/{org}/simple/{distribution}/', {'org': '_', 'distribution': 'sdxl'})]


class Operation(TypedDict, total=False):
    security: list[dict[str, list[str]]]  # each a way in; an empty one is anonymous


class PathItem(TypedDict, total=False):
    get: Operation


class Contract(TypedDict):
    """The Hub's OpenAPI document, what is read here."""
    paths: dict[str, PathItem]


def row(check: str, ok: bool, what: str) -> None:
    print(f'{check}\t{"PASS" if ok else "FAIL"}\t{what}')


def get(path: str) -> tuple[int, str, bytes]:
    req = urllib.request.Request(HUB + path, headers={'Accept': 'application/json, text/html'})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, r.headers.get('content-type', ''), r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.headers.get('content-type', ''), e.read()


if MODE in ('paths', 'paths-empty'):
    spec: Contract = yaml.safe_load(get('/openapi.yaml')[2])
    for template, args in PATHS:
        path = template.format(**args)
        status, ctype, body = get(path)
        op = (spec['paths'].get(template) or {}).get('get')
        if op is None:
            row(f'hub {path}', False, f'{template} is not in the contract document')
            continue
        security = op.get('security')
        if security and not any(not s for s in security):  # bearer only: an anonymous caller is refused
            row(f'hub {path}', status == 401, f'{status} to an anonymous caller (its contract: bearer only)')
            continue
        if MODE == 'paths-empty' and status == 404 and args:
            row(f'hub {path}', True, f'404 on an empty Hub, which has no {template.rsplit("/", 2)[-2] if template.endswith("/") else template.rsplit("/", 1)[0]}')
            continue
        row(f'hub {path}', status == 200, f'{status} {ctype} ({len(body)} bytes)' if status == 200 else f'{status}: {body[:200]!r}')
elif MODE == 'yanked':
    org, dist, version = sys.argv[3:6]
    status, _, body = get(f'/v1/index/{org}/simple/{dist}/')
    links = re.findall(r'<a [^>]*>[^<]*</a>', body.decode('utf-8', 'replace'))
    mine = [a for a in links if f'-{version}' in a or f'_{version}' in a]
    ok = status == 200 and mine and all('data-yanked' in a for a in mine)
    row('hub index data-yanked', bool(ok), f'{status}: {len(mine)} file(s) of {dist} {version}, '
        f'{sum("data-yanked" in a for a in mine)} marked data-yanked')
