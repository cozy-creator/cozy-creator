"""held.py <out> <stop>: what cozy holds on this machine's GPUs, every 0.2 s, read from their ledgers, until <stop> exists."""
import glob, os, struct, sys, time

HEADER, ROW, ROWS = 3, 9, 32
out, stop = sys.argv[1], sys.argv[2]
with open(out, "w") as file:
    while not os.path.exists(stop):
        held = 0
        for path in glob.glob("/dev/shm/cozy-varena-*/v6-*"):
            words = struct.unpack("<512Q", open(path, "rb").read(4096))
            held += sum(words[HEADER + i * ROW + 1] for i in range(ROWS) if words[HEADER + i * ROW])
        file.write(f"{time.time():.2f} {held}\n")
        file.flush()
        time.sleep(0.2)
