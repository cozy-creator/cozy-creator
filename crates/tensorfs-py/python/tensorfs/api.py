"""The public component API; storage and admission stay in the native library."""

from __future__ import annotations

import io
import os
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import TYPE_CHECKING, TypedDict

from . import _ext

if TYPE_CHECKING:
    from .api import StrPath, Token  # api.pyi's


class _Settings(TypedDict, total=False):
    """What `configure` was given (its arguments other than None)."""

    endpoint: str
    cache_dir: StrPath
    token: Token
    ca_der: bytes


_settings: _Settings = {}


def configure(*, endpoint=None, cache_dir=None, token=None, ca_der=None) -> None:
    global _settings
    _settings = {**_settings, **{k: v for k, v in locals().items() if v is not None}}


# What the caller named; None leaves the store and endpoint to the native rule (TENSORFS_HOME,
# TENSORFS_ENDPOINT and their defaults), the one every TensorFS surface follows.
def _cache(cache_dir=None) -> Path | None:
    root = cache_dir or _settings.get("cache_dir")
    return None if root is None else Path(root).expanduser()


def _endpoint(endpoint=None):
    return endpoint or _settings.get("endpoint")


def _client(cache_dir=None, endpoint=None, token=None, *, configured_token=True):
    credential = token if token is not None else _settings.get("token") if configured_token else None
    return _ext._Client(_cache(cache_dir), endpoint=_endpoint(endpoint), token=credential, ca_der=_settings.get("ca_der"))


def open(name_or_path, *, release=None, lane=None, gpus=None, endpoint=None, local_files_only=False, cache_dir=None, token=None, lazy=True):
    return _client(cache_dir, endpoint, token).open(
        os.fspath(name_or_path), release=release, lane=lane, gpus=gpus, local_files_only=local_files_only, lazy=lazy
    )


def view(header, own=(), *, endpoint=None, local_files_only=False, cache_dir=None, token=None):
    """A checkpoint whose header (cozytensors bytes) lives only in memory: an adapter view composed
    by reference. Its chunks come from the cache, else the endpoint; `own` are the objects it
    cites that no store holds. Nothing is written."""
    return _client(cache_dir, endpoint, token).view(bytes(header), [bytes(o) for o in own], local_files_only=local_files_only)


def materialize(source: str | _ext.Checkpoint, *, local_dir=None, format="cozytensors", decode=False, release=None, lane=None, components=None, cache_dir=None, local_files_only=False, token=None, endpoint=None) -> str:
    if format not in {"cozytensors", "safetensors"}:
        raise ValueError("format must be cozytensors or safetensors")
    if format == "cozytensors" and decode:
        raise ValueError("decode requires format='safetensors'")
    if isinstance(source, _ext.Checkpoint):
        if components is not None:
            raise ValueError("components selection requires a model source in cozytensors format")
        if local_dir is None:
            raise ValueError("a checkpoint requires an explicit local_dir")
        if local_files_only or any(value is not None for value in
                                   (release, lane, cache_dir, token, endpoint)):
            raise ValueError("model and client options do not apply to a checkpoint")
        return str(_ext._materialize(source, local_dir, format=format, decode=decode))
    if not isinstance(source, str):
        raise TypeError("source must be a model reference or Checkpoint")
    if format == "cozytensors":
        return str(_client(cache_dir, endpoint, token).materialize(
            (source, release, lane), components=components,
            local_dir=local_dir, local_files_only=local_files_only,
        ))
    if components is not None:
        raise ValueError("components selection is not supported for safetensors materialization")
    if local_dir is None:
        raise ValueError("safetensors materialization requires an explicit local_dir")
    checkpoint = _client(cache_dir, endpoint, token).open(
        source, release=release, lane=lane, local_files_only=local_files_only,
    )
    try:
        return str(_ext._materialize(checkpoint, local_dir, format=format, decode=decode))
    finally:
        checkpoint.close()


def _model_id(model_id, repo_id, *, required=True):
    # Compatibility for Runtime 0.45 and existing package locks; planned removal in 0.25.0
    # after the supported Runtime/package floor uses model_id.
    if model_id is not None and repo_id is not None:
        raise TypeError("use model_id; model_id and legacy repo_id cannot both be supplied")
    selected = model_id if model_id is not None else repo_id
    if required and selected is None:
        raise TypeError("model_id is required")
    return selected


def download_file(model_id=None, filename=None, *, repo_id=None, release=None, lane=None, cache_dir=None, local_dir=None, local_files_only=False, token=None, endpoint=None) -> str:
    model_id = _model_id(model_id, repo_id)
    return str(_client(cache_dir, endpoint, token).download_file(
        model_id, filename, release=release, lane=lane, local_dir=local_dir, local_files_only=local_files_only,
    ))


def upload_folder(*, model_id=None, folder_path, lane: str, repo_id=None, token=None, endpoint=None, cache_dir=None) -> str:
    model_id = _model_id(model_id, repo_id)
    return _client(cache_dir, endpoint, token).upload_folder(Path(folder_path), model_id, lane)


def convert_to_cozytensors(source: str | Sequence[str], *, converter=None, source_variant=None, cache_dir=None, token=None, progress=None):
    return _client(cache_dir, token=token, configured_token=False).convert_to_cozytensors(
        source, converter=converter, source_variant=source_variant, progress=progress,
    )


class CheckpointFile(io.RawIOBase):
    """The checkpoint as one read-only, seekable file: its header, then its chunks and assets in
    header order at 4 KiB-aligned offsets, assembled from the store on demand; nothing is written."""

    def __init__(self, native):
        super().__init__()
        self._native = native

    def readable(self) -> bool:
        return True

    def seekable(self) -> bool:
        return True

    def readinto(self, buffer) -> int:
        return self._native.readinto(buffer)

    def seek(self, offset: int, whence: int = io.SEEK_SET) -> int:
        return self._native.seek(offset, whence)

    def tell(self) -> int:
        return self._native.tell()

    def close(self) -> None:
        self._native.close()
        super().close()


class Endpoint:
    def __init__(self, url, token=None, *, ca_der=None, cache_dir=None):
        self._native = _ext.Endpoint(url, token, ca_der=ca_der, cache_dir=_cache(cache_dir))

    def resolve(self, model_id=None, *, repo_id=None, release=None, lane=None, gpus=None, plain=False):
        return self._native.resolve(_model_id(model_id, repo_id), release=release, lane=lane, gpus=gpus, plain=plain)


class ComponentWriter:
    def __init__(self, native):
        self._native = native

    def put(self, key, data, *, dtype=None, shape=None) -> None:
        self._native.put_array(key, data, dtype=dtype, shape=shape)

    def put_encoded(self, key, *, dtype, shape, scheme, params: Mapping, parts: Mapping) -> None:
        self._native.put_encoded(key, dtype, shape, scheme, parts, dict(params))

    def copy(self, key, source, *, as_key=None) -> None:
        self._native.copy_component(key, source, as_key)

    def asset(self, name, data) -> None:
        self._native.asset(name, data)

    def order(self, keys) -> None:
        self._native.order(keys)

    def has(self, key) -> bool:
        return self._native.has(key)


class Writer:
    def __init__(self, dest=None, *, base=None, resume=None, model_id=None, lane=None, repo_id=None, endpoint=None, token=None, cache_dir=None):
        self._native = _ext.Writer(dest, base=base, resume=resume, cache_dir=_cache(cache_dir), model_id=_model_id(model_id, repo_id, required=False), lane=lane, endpoint=_endpoint(endpoint), token=token if token is not None else _settings.get("token"), ca_der=_settings.get("ca_der"))
        self._resume = resume
        self._committed = None
        self.local_checkpoint = None
        self._failed = None
        self.metadata = {}

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, traceback):
        if exc_type is None:
            self.commit()
        elif self._resume is None:
            self.abandon()

    def pipeline(self, cls=None, config: Mapping | None = None) -> None:
        self._native.pipeline(cls, dict(config) if config is not None else {})

    def component(self, name: str, *, cls=None, config: Mapping | None = None, config_file=None) -> ComponentWriter:
        return ComponentWriter(self._native.component(name, cls, dict(config) if config is not None else {}, config_file))

    def commit(self):
        if self._failed is not None:
            raise self._failed
        if self._committed is None:
            for key, value in self.metadata.items():
                self._native.set_metadata(key, value)
            try:
                ck = self._native.commit()
            except _ext.ConflictError:
                raise  # A native conversion still owns the Writer; retry after its join.
            except _ext.Error as error:
                self.local_checkpoint = getattr(error, "checkpoint", None)
                self._failed = error
                raise
            self._committed = ck
        return self._committed

    def abandon(self) -> None:
        self._native.abandon()

    def has(self, component, key) -> bool:
        return self._native.has(component, key)

    def normalize_bf16(self, source, tensors, *, max_buffer_bytes: int, max_new_bytes: int, control):
        """Write plain BF16 through the native reader, RNE cast, quality checks and journal.

        Supply the same full selection on every attempt, including completed tensors.
        The native call owns the Writer until it returns; this does not commit it.
        """
        return self._native.normalize_bf16(source, tensors, max_buffer_bytes=max_buffer_bytes, max_new_bytes=max_new_bytes, control=control)

    def quantize(self, source, tensors, *, max_buffer_bytes: int, max_new_bytes: int, control, max_relative_frobenius: float | None = None, calibration=None, component: str | None = None):
        """Encode a complete resolved selection natively, without committing this Writer.

        `control` can cancel/observe from another thread; observations never drive the loop.
        Source, calibration snapshot and full recipe must remain identical on resume.
        """
        return self._native.quantize(source, tensors, max_buffer_bytes=max_buffer_bytes, max_new_bytes=max_new_bytes, control=control, max_relative_frobenius=max_relative_frobenius, calibration=calibration, component=component)
