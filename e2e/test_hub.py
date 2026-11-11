"""Verify that synthetic publication tests target their isolated local Hub."""
from __future__ import annotations

import argparse
from collections.abc import Callable
import http.client
import ipaddress
import json
import os
import ssl
import sys
from typing import cast
from urllib.parse import urlsplit


class IsolationError(ValueError):
    pass


def _origin(value: str, label: str, *, issuer: bool = False) -> tuple[str, str, int]:
    try:
        parsed = urlsplit(value)
        host = parsed.hostname or ""
        port = parsed.port or (443 if parsed.scheme == "https" else 80)
    except ValueError as error:
        raise IsolationError(f"{label} is not a valid test origin") from error
    if parsed.scheme not in {"http", "https"} or not host or parsed.username is not None or parsed.password is not None:
        raise IsolationError(f"{label} must be an HTTP(S) loopback origin without credentials")
    try:
        loopback = host == "localhost" or ipaddress.ip_address(host).is_loopback
    except ValueError:
        loopback = False
    if not loopback:
        raise IsolationError(f"{label} is not loopback; synthetic tests must never publish to production")
    if parsed.query or parsed.fragment or (not issuer and parsed.path not in {"", "/"}):
        raise IsolationError(f"{label} must not contain a path, query or fragment")
    return parsed.scheme, host, port


def metadata(hub: str) -> dict[str, object]:
    scheme, host, port = _origin(hub, "test Hub URL")
    connection: http.client.HTTPConnection
    if scheme == "https":
        connection = http.client.HTTPSConnection(host, port, timeout=15, context=ssl.create_default_context())
    else:
        connection = http.client.HTTPConnection(host, port, timeout=15)
    try:
        connection.request("GET", "/.well-known/oauth-protected-resource")
        response = connection.getresponse()
        if response.status != 200:
            raise IsolationError(f"test Hub metadata returned HTTP {response.status}; redirects are not followed")
        value: object = json.loads(response.read(65536))
    except (OSError, http.client.HTTPException, ValueError) as error:
        raise IsolationError(f"could not verify test Hub metadata: {error}") from error
    finally:
        connection.close()
    if not isinstance(value, dict):
        raise IsolationError("test Hub metadata is not an object")
    return cast(dict[str, object], value)


def require_test_hub(
    hub: str,
    expected_origin: str,
    fetch: Callable[[str], dict[str, object]] = metadata,
) -> None:
    _origin(hub, "test Hub URL")
    expected = _origin(expected_origin, "COZY_TEST_HUB_ORIGIN")
    document = fetch(hub)
    resource = document.get("resource")
    if not isinstance(resource, str) or _origin(resource, "Hub resource") != expected:
        raise IsolationError("Hub resource differs from the isolated fixture origin")
    issuers = document.get("authorization_servers")
    if not isinstance(issuers, list) or not issuers:
        raise IsolationError("test Hub metadata names no authorization server")
    for issuer in issuers:
        if not isinstance(issuer, str) or _origin(issuer, "Hub authorization server", issuer=True) != expected:
            raise IsolationError("Hub authorization server differs from the isolated fixture origin")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hub", required=True)
    parser.add_argument("--expected-origin", default=os.environ.get("COZY_TEST_HUB_ORIGIN", ""))
    args = parser.parse_args()
    try:
        require_test_hub(args.hub, args.expected_origin)
    except IsolationError as error:
        print(f"test Hub required: {error}. Use the isolated e2e/skew runner.", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
