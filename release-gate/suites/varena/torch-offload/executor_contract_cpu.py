"""Actual installed SDK wire hard cut; no Torch import, fake runtime or GPU work."""
import json

from tensorfs import executor

assert "room" not in executor.COMMANDS
assert "Room" not in vars(executor)
try:
    executor.decode_command(b'{"cmd":"room","device":0,"target":1,"held":0}')
except executor.Malformed as error:
    refusal = str(error)
else:
    raise AssertionError("removed Room command was accepted")
frames = []
for degree in (1, 2, 4):
    owners = [dict(uuid=f"GPU-13800000-0000-0000-0000-{rank:012}", directory="/dev/shm/varena-wire-only", shared=False, priority=rank) for rank in range(degree)]
    frame = dict(cmd="start", devices=",".join(str(rank) for rank in range(degree)), sequence_parallel_degree=degree, cooperation=owners)
    command = executor.decode_command(json.dumps(frame).encode())
    assert isinstance(command, executor.Start)
    assert len(command.cooperation) == degree
    assert [value.uuid for value in command.cooperation] == [value["uuid"] for value in owners]
    for old in (owners[0], None):
        try:
            executor.decode_command(json.dumps(dict(cmd="start", cooperation=old)).encode())
        except executor.Malformed:
            pass
        else:
            raise AssertionError("non-vector cooperation was accepted")
    frames.append(degree)
print(json.dumps(dict(passed=True, room_refusal=refusal, rank_counts=frames, gpu_executed=False)))
