"""The owned harness records and the JSON views consumed by its verification gate."""
import json
from collections.abc import Mapping
from pathlib import Path
from typing import NotRequired, TypedDict


class ProcessIdentity(TypedDict):
    birth: str
    exe: str
    home: str


class Ownership(TypedDict):
    home: str
    state: str


class ProcessRecord(ProcessIdentity):
    state: str
    args: list[str]


ProcessSnapshot = dict[int, ProcessRecord]


class MaterializedComponent(TypedDict):
    input: str
    sha256: NotRequired[str]
    local_path: NotRequired[str]


class CleanupReceipt(TypedDict):
    processes: ProcessSnapshot
    remaining_pids: list[int]
    containers_removed: list[str]
    remaining_containers: list[str]
    remaining_created_containers: NotRequired[list[str]]
    error: NotRequired[str]
    tree_error: NotRequired[str]


class HarnessReceipt(TypedDict):
    mode: str
    home: str
    owner: str
    device: str
    gpu_inventory: list[str]
    command: list[str]
    cozy_sha256: str
    hub_source_sha: str
    runtime: str
    tensorfs: str
    existing_cozy_processes: ProcessSnapshot
    requested_components: NotRequired[dict[str, MaterializedComponent]]
    created_container_ids: NotRequired[list[str]]
    cleanup: NotRequired[CleanupReceipt]
    existing_cozy_processes_unchanged: NotRequired[bool]
    fixture_status: NotRequired[int]
    private_home_removed: NotRequired[bool]


class Candidate(TypedDict):
    version: str
    sha256: str | None


class Software(TypedDict):
    runtime: str
    tensorfs: str


class InstalledCohort(Software):
    tensorfs_sha256: str
    runtime_sha256: NotRequired[str]


class ArithmeticResult(TypedDict):
    result: int
    device: str


class GPUIdentity(TypedDict):
    uuid: str


class Execution(TypedDict):
    gpus: list[GPUIdentity]


class Measurements(TypedDict):
    software: Software
    execution: NotRequired[Execution]


class RunRecord(TypedDict):
    status: str
    result: ArithmeticResult
    measurements: Measurements


class PayloadFingerprint(TypedDict):
    path: str
    sha256: str
    wheel_entry: str
    wheel_sha256: str


class ProductResult(TypedDict):
    device: str
    products: list[float]


class SDKReceipt(TypedDict):
    status: str
    python: str
    wheel: str
    wheel_sha256: str
    sdk_version: NotRequired[str]
    runtime_version: NotRequired[str]
    torch_version: NotRequired[str]
    revision: NotRequired[str]
    native_payloads: NotRequired[dict[str, PayloadFingerprint]]
    cpu: NotRequired[ProductResult]
    gpu: NotRequired[ProductResult]
    gpu_uuid: NotRequired[str]
    error: NotRequired[str]


class SDKVerified(TypedDict):
    status: str
    gpu_uuid: str
    sdk_version: str
    runtime_version: str
    wheel_sha256: str
    cpu: ProductResult
    gpu: ProductResult
    native_payloads: dict[str, PayloadFingerprint]


def object_map(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        raise TypeError(f'expected JSON object, got {type(value).__name__}')
    result: dict[str, object] = {}
    for key, item in value.items():
        if not isinstance(key, str):
            raise TypeError('JSON object key is not a string')
        result[key] = item
    return result


def sequence(value: object) -> list[object]:
    if not isinstance(value, list):
        raise TypeError('expected JSON array')
    return list(value)


def text(value: object) -> str:
    if not isinstance(value, str):
        raise TypeError('expected JSON string')
    return value


def read_object(path: Path) -> dict[str, object]:
    value: object = json.loads(path.read_text())
    return object_map(value)


def software(value: object) -> Software:
    record = object_map(value)
    return {'runtime': text(record['runtime']), 'tensorfs': text(record['tensorfs'])}


def read_run(path: Path) -> RunRecord:
    record = read_object(path)
    result = object_map(record['result'])
    number = result['result']
    if not isinstance(number, int):
        raise TypeError('arithmetic result is not an integer')
    measured = object_map(record['measurements'])
    measurements: Measurements = {'software': software(measured['software'])}
    if 'execution' in measured:
        execution = object_map(measured['execution'])
        measurements['execution'] = {'gpus': [{'uuid': text(object_map(gpu)['uuid'])}
                                             for gpu in sequence(execution['gpus'])]}
    return {'status': text(record['status']), 'result': {'result': number, 'device': text(result['device'])},
            'measurements': measurements}


def read_installed(path: Path) -> InstalledCohort:
    found: InstalledCohort | None = None
    for line in path.read_text().splitlines():
        if not line.startswith('{'):
            continue
        value: object = json.loads(line)
        event = object_map(value)
        if event.get('type') != 'run.completed':
            continue
        target = object_map(object_map(object_map(event['run'])['result'])['to'])
        found = {'runtime': text(target['runtime']), 'tensorfs': text(target['tensorfs']),
                 'tensorfs_sha256': text(target['tensorfs_sha256'])}
        if 'runtime_sha256' in target:
            found['runtime_sha256'] = text(target['runtime_sha256'])
    if found is None:
        raise ValueError('candidate update has no completed installed cohort')
    return found


def products(value: object) -> ProductResult:
    record = object_map(value)
    numbers: list[float] = []
    for item in sequence(record['products']):
        if not isinstance(item, (int, float)):
            raise TypeError('SDK product is not numeric')
        numbers.append(float(item))
    return {'device': text(record['device']), 'products': numbers}


def payload(value: object) -> PayloadFingerprint:
    record = object_map(value)
    return {'path': text(record['path']), 'sha256': text(record['sha256']),
            'wheel_entry': text(record['wheel_entry']), 'wheel_sha256': text(record['wheel_sha256'])}


def read_sdk(path: Path) -> SDKVerified:
    record = read_object(path)
    fingerprints = object_map(record['native_payloads'])
    return {'status': text(record['status']), 'gpu_uuid': text(record['gpu_uuid']),
            'sdk_version': text(record['sdk_version']), 'runtime_version': text(record['runtime_version']),
            'wheel_sha256': text(record['wheel_sha256']), 'cpu': products(record['cpu']), 'gpu': products(record['gpu']),
            'native_payloads': {name: payload(item) for name, item in fingerprints.items()}}
