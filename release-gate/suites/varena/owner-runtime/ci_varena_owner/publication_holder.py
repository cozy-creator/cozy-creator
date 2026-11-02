"""Hold only the real OFD publication lock; no table write, row or CUDA context."""
import fcntl
import json
import os
import struct
import sys

config = json.loads(sys.argv[1])
fd = os.open(config['path'], os.O_RDWR | os.O_CLOEXEC)
try:
    identity = os.fstat(fd)
    assert identity.st_dev == config['device'] and identity.st_ino == config['inode']
    lock = struct.pack('hh4xqqi4x', fcntl.F_WRLCK, os.SEEK_SET, 0, 1, 0)
    fcntl.fcntl(fd, fcntl.F_OFD_SETLKW, lock)
    print(json.dumps({'publication_held': True, 'pid': os.getpid(),
                      'device': identity.st_dev, 'inode': identity.st_ino}), flush=True)
    assert sys.stdin.readline().strip() == 'stop'
finally:
    os.close(fd)
print(json.dumps({'publication_released': True}), flush=True)
