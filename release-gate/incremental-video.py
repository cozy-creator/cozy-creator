"""cl-1210's proof, what incremental-video.sh computes off the pod.

    incremental-video.py tap NDJSON OUT --stop FILE
    incremental-video.py report DIR
    incremental-video.py pictures BEFORE AFTER WHOLE --seams F[,F...] --joins F[,F...] --out DIR

`tap` runs beside an arm until FILE exists: each event of the run as it reaches this computer (its clock, ms), and five times a second
the TCP payload received from each peer (`ss -tin`'s `bytes_received`, each socket's last reading kept) and the interfaces' received
bytes (/proc/net/dev). Calcifer's own account of a read is the run's record (`measurements.transfer`); this is the wire's, which takes
nobody's word.

`report` is iv.json: each arm's revisions (length, what it kept of the one before as the run's events said, `appended_from`; the
payload this computer received from the machine between the revision's event and its landing; ffprobe; a full decode), the record's
account beside them, its last segment's decode, and its last denoise to the take on disk, both read on this computer's clock.

`pictures` is one JSON line. BEFORE is the shot as today makes it (its last segment decodes the whole shot again; `-`: none), AFTER as
cl-1210 makes it (each segment decodes its windows once, the take their join), WHOLE the whole-shot decode of AFTER's own latents
(minimax-h3 `vae_round_trip`; `-`: none). Frames are 0-based; `seams` are where the shot's segments meet, `joins` where AFTER's parts meet.
- `after_vs_whole`: per-frame luma PSNR and SSIM (ffmpeg's), the least, and the least within 6 frames of a seam or join: the same latents
  decoded once in parts or once whole; any difference is the H.264 encode's. `near` is each frame from 6 before to 6 after.
- `after_vs_before`: the same over segment 1 short of its last second, which both arms make alike (same seed and conditioning), and over
  the rest, whose conditioning differs (the next segment's picture of where the last one ends).
- `steps`: at each seam, the change between consecutive frames there as a ratio to the arm's median change, both arms (a stepped
  picture at a join scores high; run 20751's was luma +3.7).
- `sound`: each arm's sound at each seam, the largest sample-to-sample step within 5 ms of it over the arm's 99.9th-percentile step
  (a click scores past 1); its length against its pictures' (drift, ms); and over segment 1, the lag (ms) and correlation of AFTER's sound
  against BEFORE's. `butts`: within 20 ms of each join, the largest sample step, where (ms from the picture's join) and over that
  percentile: sound butted with no crossfade jumps there, up to a few ms short of the picture's join.
- `brightness`: each segment's mean luma, and at each seam the 3 frames before against the 3 after, both arms (AFTER at its joins too).
- A strip a seam and a join (`strip-<frame>.png`): frames f-2..f+2, BEFORE over AFTER over WHOLE; `pair-<frame>.png`: AFTER's frames
  f-1 and f at full size and their difference eight times over; `sound-<frame>.png`: the second around it as a spectrogram, BEFORE over
  AFTER (a click is a vertical line).
"""

import json
import os
import re
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

FPS = 24
RATE = 32000


def run(*args: str) -> bytes:
    return subprocess.run(["nice", "-n", "19", *args], capture_output=True, check=True).stdout


# ---- tap

LOCAL = ("127.", "[::1]", "[::ffff:127.")


def sockets() -> dict[str, int]:
    """Each TCP socket's payload received so far, by `local peer`; loopback left out."""
    said = subprocess.run(["ss", "-tinH"], capture_output=True, text=True).stdout
    rows: dict[str, int] = {}
    key = None
    for line in said.splitlines():
        if line[:1] not in (" ", "\t"):
            parts = line.split()
            key = f"{parts[3]} {parts[4]}" if len(parts) >= 5 and not parts[4].startswith(LOCAL) else None
        elif key and (got := re.search(r"bytes_received:(\d+)", line)):
            rows[key] = int(got.group(1))
    return rows


def received() -> int:
    """Bytes received on every interface but loopback, headers and all."""
    total = 0
    for line in Path("/proc/net/dev").read_text().splitlines()[2:]:
        name, _, rest = line.partition(":")
        if name.strip() != "lo" and rest.split():
            total += int(rest.split()[0])
    return total


def kept(event: dict[str, object]) -> dict[str, object] | None:
    """What of an event the report reads; none for its progress."""
    kind = str(event.get("type") or "")
    if kind.startswith("output_item."):
        item = event.get("item") if isinstance(event.get("item"), dict) else {}
        assert isinstance(item, dict)
        name = str(item.get("name") or str(event.get("item_id") or "").split("/", 1)[-1])
        return {"type": kind, "name": name, "rev": item.get("rev", event.get("rev")), "length": item.get("length", event.get("length")),
                "appended_from": event.get("appended_from"), "status": item.get("status"), "landed": bool(item.get("url"))}
    if kind == "run.log":
        return {"type": kind, "level": event.get("level"), "text": event.get("text"), "at_ms": event.get("at_ms")}
    if kind in ("run.created", "run.in_progress", "run.completed", "run.failed", "run.canceled"):
        return {"type": kind}
    return None


def closings(ended: dict[str, int], lock: threading.Lock) -> subprocess.Popen[str]:
    """Each TCP socket's payload received as it closes (`ss -E`), into `ended` by `local peer`: what came after the tap's last
    sample of a lane that closed before the next one."""
    follower = subprocess.Popen(["ss", "-EtinH"], stdout=subprocess.PIPE, text=True)

    def follow() -> None:
        key = None
        assert follower.stdout is not None
        for line in follower.stdout:
            if line[:1] not in (" ", "\t"):
                parts = line.split()
                key = f"{parts[3]} {parts[4]}" if len(parts) >= 5 and not parts[4].startswith(LOCAL) else None
            elif key and (got := re.search(r"bytes_received:(\d+)", line)):
                with lock:
                    ended[key] = int(got.group(1))

    threading.Thread(target=follow, daemon=True).start()
    return follower


def tap(ndjson: str, out: str, stop: str) -> None:
    seen: dict[str, int] = {}
    closed: dict[str, int] = {}
    written: dict[str, int] = {}
    ended: dict[str, int] = {}
    lock = threading.Lock()
    follower = closings(ended, lock)
    at = 0
    with open(out, "a") as log:
        while True:
            now = int(time.time() * 1000)
            try:
                with open(ndjson, "rb") as stream:
                    if os.fstat(stream.fileno()).st_size < at:
                        at = 0  # written anew
                    stream.seek(at)
                    chunk = stream.read()
            except OSError:
                chunk = b""
            whole = chunk[: chunk.rfind(b"\n") + 1]
            at += len(whole)
            for line in whole.splitlines():
                try:
                    event = kept(json.loads(line))
                except (ValueError, AssertionError):
                    continue
                if event is not None:
                    log.write(json.dumps({"t": now, "event": event}) + "\n")
            now_open = sockets()
            with lock:
                gone, _ = dict(ended), ended.clear()
            for key, count in gone.items():
                if key not in now_open and count >= seen.get(key, 0):
                    seen[key] = count
            for key, count in now_open.items():
                # A port taken again by a new connection counts on from what the old one received.
                if count < seen.get(key, 0):
                    closed[key] = closed.get(key, 0) + seen[key]
                seen[key] = count
            peers: dict[str, int] = {}
            for key, count in seen.items():
                peer = key.split(" ")[1].rsplit(":", 1)[0]
                peers[peer] = peers.get(peer, 0) + count + closed.get(key, 0)
            # Each socket's count too, as it changes: the report tells the output reads' bulk lanes from the control connection.
            socks = {key: count + closed.get(key, 0) for key, count in seen.items()}
            moved = {key: count for key, count in socks.items() if written.get(key) != count}
            written.update(moved)
            log.write(json.dumps({"t": now, "tcp": peers, "socks": moved, "net": received()}) + "\n")
            log.flush()
            if os.path.exists(stop):
                follower.terminate()
                return
            time.sleep(0.2)


# ---- report

def lines(path: Path) -> list[str]:
    return [line for line in path.read_text().splitlines() if line] if path.exists() else []


def addresses(host: str) -> set[str]:
    try:
        return {str(info[4][0]) for info in socket.getaddrinfo(host, None)} | {host}
    except OSError:
        return {host}


def arm(out: Path, name: str) -> dict[str, object]:
    revs = [line.split("\t") for line in lines(out / f"iv-{name}.revs")]
    record = json.loads((out / f"iv-{name}.json").read_text()) if (out / f"iv-{name}.json").exists() else {}
    machine = json.loads((out / f"iv-{name}.machine").read_text()) if (out / f"iv-{name}.machine").exists() else {}
    tapped = [json.loads(line) for line in lines(out / f"iv-{name}.tap")]
    events = [(row["t"], row["event"]) for row in tapped if "event" in row]
    samples = [row for row in tapped if "tcp" in row]
    # The machine among the peers: by its address, else the one most came from, which is said.
    host = (out / "address").read_text().strip() if (out / "address").exists() else ""
    wanted = {peer.strip("[]").removeprefix("::ffff:") for peer in addresses(host)} if host else set()
    volume: dict[str, int] = {}
    for sample in samples:
        for peer, count in sample["tcp"].items():
            volume[peer] = max(volume.get(peer, 0), count)
    peers = [peer for peer in volume if peer.strip("[]").removeprefix("::ffff:") in wanted]
    by = "address"
    if not peers and volume:
        peers, by = [max(volume, key=lambda peer: volume[peer])], "volume"

    def tcp(sample: dict[str, object]) -> int:
        counts = sample["tcp"]
        assert isinstance(counts, dict)
        return sum(int(counts.get(peer, 0)) for peer in peers)

    def window(start: int, end: int) -> tuple[int | None, int | None]:
        """What came from the machine, and on every interface, from the last sample two periods before `start` to the first at `end`."""
        first = [sample for sample in samples if sample["t"] <= start - 400] or samples[:1]
        last = [sample for sample in samples if sample["t"] >= end]
        if not first or not last:
            return None, None
        return tcp(last[0]) - tcp(first[-1]), int(last[0]["net"]) - int(first[-1]["net"])

    # Each socket to the machine, its count carried from its last change. Calcifer reads a machine on one HTTP/2 connection for
    # control calls, the event stream and status, and reads outputs on bulk HTTP/1.1 lanes of their own (machine_client.rs): the
    # control socket is the one whose bytes kept coming over the longest span, every other socket to the machine a bulk lane.
    running: dict[str, int] = {}
    spans: dict[str, list[int]] = {}
    for sample in samples:
        for key, count in (sample.get("socks") or {}).items():
            if key.split(" ")[1].rsplit(":", 1)[0] in peers:
                running[key] = int(count)
                spans.setdefault(key, [sample["t"], sample["t"]])[1] = sample["t"]
        sample["machine_socks"] = dict(running)
    control = max(spans, key=lambda key: spans[key][1] - spans[key][0]) if spans else None

    def split(start: int, end: int) -> dict[str, int] | None:
        """Each machine socket's bytes over `window`'s samples, those that moved."""
        first = [sample for sample in samples if sample["t"] <= start - 400] or samples[:1]
        last = [sample for sample in samples if sample["t"] >= end]
        if not spans or not first or not last:
            return None
        before, after = first[-1]["machine_socks"], last[0]["machine_socks"]
        return {key: after[key] - before.get(key, 0) for key in after if after[key] != before.get(key, 0)}

    video = [(t, event) for t, event in events if str(event.get("type", "")).startswith("output_item.") and event.get("name") == "video"]
    said = {int(event["rev"]): t for t, event in reversed(video) if event["type"] in ("output_item.added", "output_item.delta") and event.get("rev")}
    kept_of = {int(event["rev"]): int(event.get("appended_from") or 0) for _, event in video if event["type"] == "output_item.delta" and event.get("rev")}
    landed = {int(event["rev"]): t for t, event in reversed(video) if event["type"] == "output_item.landed" and event.get("rev")}
    sizes = {int(event["rev"]): int(event["length"]) for _, event in video if event["type"] == "output_item.landed" and event.get("rev") and event.get("length")}
    # Every revision that landed, as the run's events say; its probe and decode where the watcher took its file (none: not probed).
    watched = {int(rev_text): (int(at), json.loads(probe), errors.strip()) for rev_text, at, size, probe, errors in revs}
    rows = []
    for rev in sorted(set(sizes) | set(watched)):
        length = sizes.get(rev) or int(next(size for rev_text, _, size, _, _ in revs if int(rev_text) == rev))
        wire, net = window(said[rev], landed[rev]) if rev in said and rev in landed else (None, None)
        by_socket = split(said[rev], landed[rev]) if rev in said and rev in landed else None
        at, probe, errors = watched.get(rev, (None, {}, None))
        rows.append({"rev": rev, "length": length, "appended_from": kept_of.get(rev, 0), "expected_read": length - kept_of.get(rev, 0),
                     "wire": wire, "wire_interfaces": net, "said_ms": said.get(rev), "landed_ms": landed.get(rev), "watched_ms": at,
                     "wire_bulk": None if by_socket is None else sum(v for key, v in by_socket.items() if key != control),
                     "wire_control": None if by_socket is None else by_socket.get(control, 0), "wire_sockets": by_socket,
                     "probe": probe, "decode_errors": errors})
    calls = record.get("calls") or []
    last = calls[-1] if calls else {}
    stages = ((last.get("measurements") or {}).get("attribution") or {}).get("stages") or {}
    logs = [(t, str(event.get("text") or "")) for t, event in events if event.get("type") == "run.log"]
    geometry = [dict(re.findall(r"(\w+)=(\S+)", text)) for _, text in logs if "h3 output geometry" in text]
    # The last denoise ends where the last segment's soundtrack decode begins: its stage line, as it reached this computer.
    denoised = [t for t, text in logs if text.endswith("decode_audio")]
    took = (record.get("measurements") or {}).get("transfer", {}).get("video") or {}
    lanes = sorted({str(model.get("lane")) for call in [record, *calls] for model in ((call.get("measurements") or {}).get("models") or []) if model.get("lane")})
    wires = [row["wire"] for row in rows]
    return {
        "run": record.get("number"), "status": record.get("status"), "runtime": machine.get("runtime"), "tensorfs": machine.get("tensorfs"),
        "cozy": machine.get("version"), "lanes": lanes, "peer": {"by": by, "peers": peers}, "revisions": rows,
        "take": rows[-1]["length"] if rows else 0,
        "expected_total": sum(int(row["expected_read"]) for row in rows),
        "wire_total": sum(wires) if rows and all(wire is not None for wire in wires) else None,
        "wire_bulk_total": sum(row["wire_bulk"] for row in rows) if rows and all(row["wire_bulk"] is not None for row in rows) else None,
        "sockets": {"classified": "control: the socket to the machine whose bytes kept coming over the longest span (its one HTTP/2 "
                                  "connection: control calls, events, status); bulk: every other socket to it (HTTP/1.1 output lanes)",
                    "control": control,
                    "each": [{"socket": key, "class": "control" if key == control else "bulk", "first_ms": first, "last_ms": last,
                              "bytes": running.get(key, 0)} for key, (first, last) in sorted(spans.items(), key=lambda item: item[1][0])]},
        "record": {"read": took.get("read"), "reused": took.get("reused")},
        "warnings": [warning.get("code") for warning in record.get("warnings") or []],
        "last_segment": {"frames": int(geometry[-1].get("frames", 0)) if geometry else 0,
                         "delivered_frames": int(geometry[-1].get("delivered_frames", geometry[-1].get("frames", 0))) if geometry else 0,
                         "decode_video_ms": round((stages.get("decode_video") or {}).get("total_ms", 0)),
                         "stages_ms": {stage: round(value.get("total_ms", 0)) for stage, value in stages.items() if isinstance(value, dict)}},
        "last_denoise_to_take_ms": rows[-1]["landed_ms"] - denoised[-1] if rows and denoised and rows[-1]["landed_ms"] else None,
    }


# ---- pictures

def frames(path: str) -> int:
    probe = json.loads(run("ffprobe", "-v", "error", "-select_streams", "v:0", "-count_frames", "-show_entries", "stream=nb_read_frames",
                           "-of", "json", path))
    return int(probe["streams"][0]["nb_read_frames"])


def compare(one: str, two: str, out: Path, tag: str) -> tuple[list[float], list[float]]:
    """Per-frame luma PSNR and SSIM of `two` against `one` (ffmpeg's ssim and psnr filters)."""
    ssim_log, psnr_log = out / f"{tag}-ssim.log", out / f"{tag}-psnr.log"
    run("ffmpeg", "-nostdin", "-v", "error", "-i", two, "-i", one, "-lavfi",
        f"[0:v][1:v]ssim=stats_file={ssim_log};[0:v][1:v]psnr=stats_file={psnr_log}", "-f", "null", "-")
    ssim = [float(m.group(1)) for m in re.finditer(r"Y:([0-9.]+)", ssim_log.read_text())]
    psnr = [99.0 if m.group(1) == "inf" else float(m.group(1)) for m in re.finditer(r"psnr_y:([0-9.inf]+)", psnr_log.read_text())]
    return psnr, ssim


def summary(psnr: list[float], ssim: list[float], near: list[int], span: range | None = None) -> dict[str, object]:
    import numpy as np

    keep = [i for i in range(min(len(psnr), len(ssim))) if span is None or i in span]
    if not keep:
        return {}
    close = [i for i in keep if any(abs(i - f) <= 6 for f in near)]
    worst = min(keep, key=lambda i: ssim[i])
    return {"frames": len(keep), "psnr_mean": round(float(np.mean([psnr[i] for i in keep])), 2), "psnr_least": round(min(psnr[i] for i in keep), 2),
            "ssim_mean": round(float(np.mean([ssim[i] for i in keep])), 4), "ssim_least": round(ssim[worst], 4), "ssim_least_at": worst,
            "near_ssim_least": round(min(ssim[i] for i in close), 4) if close else None,
            "near_psnr_least": round(min(psnr[i] for i in close), 2) if close else None,
            "near": {str(f): [{"frame": i, "psnr": round(psnr[i], 2), "ssim": round(ssim[i], 4)} for i in keep if abs(i - f) <= 6] for f in near}}


def gray(path: str):  # type: ignore[no-untyped-def]
    import numpy as np

    raw = run("ffmpeg", "-nostdin", "-v", "error", "-i", path, "-vf", "scale=224:128:flags=area,format=gray", "-f", "rawvideo", "-")
    return np.frombuffer(raw, np.uint8).reshape(-1, 128, 224).astype(np.float64)


def steps(path: str, seams: list[int]) -> list[dict[str, float]]:
    import numpy as np

    change = np.abs(np.diff(gray(path), axis=0)).mean(axis=(1, 2))
    median = max(float(np.median(change)), 0.5)
    return [{"at": f, "ratio": round(float(change[max(0, f - 2):f + 2].max()) / median, 2)} for f in seams if 0 < f < len(change)]


def brightness(path: str, marks: list[int]) -> dict[str, object]:
    """Mean luma (0-255, as decoded) of each stretch between `marks`, and at each mark the 3 frames before against the 3 after."""
    import numpy as np

    luma = gray(path).mean(axis=(1, 2))
    bounds = [0, *[f for f in marks if 0 < f < len(luma)], len(luma)]
    stretches = [round(float(luma[a:b].mean()), 2) for a, b in zip(bounds, bounds[1:], strict=False) if b > a]
    at = []
    for f in marks:
        if 3 <= f <= len(luma) - 3:
            ahead, behind = float(luma[f - 3:f].mean()), float(luma[f:f + 3].mean())
            at.append({"at": f, "before": round(ahead, 2), "after": round(behind, 2), "step": round(behind - ahead, 2)})
    return {"segments": stretches, "joins": at}


def sound(path: str):  # type: ignore[no-untyped-def]
    import numpy as np

    return np.frombuffer(run("ffmpeg", "-nostdin", "-v", "error", "-i", path, "-vn", "-ac", "1", "-ar", str(RATE), "-f", "f32le", "-"), np.float32)


def clicks(wave, seams: list[int]) -> list[dict[str, float | None]]:  # type: ignore[no-untyped-def]
    import numpy as np

    step = np.abs(np.diff(wave))
    norm = max(float(np.percentile(step, 99.9)), 1e-6) if step.size else 1e-6
    out: list[dict[str, float | None]] = []
    for f in seams:
        at = f * RATE // FPS
        near = step[max(0, at - RATE // 200):at + RATE // 200]
        out.append({"at": f, "click": round(float(near.max()) / norm, 2) if near.size else None})
    return out


def butts(wave, marks: list[int]) -> list[dict[str, float | None]]:  # type: ignore[no-untyped-def]
    """Within 20 ms of each picture join, the largest sample-to-sample step (full scale 1), where it is (ms from the join), and that
    over the arm's 99.9th-percentile step: a soundtrack butted with no crossfade jumps there, even short of the picture's join."""
    import numpy as np

    step = np.abs(np.diff(wave))
    norm = max(float(np.percentile(step, 99.9)), 1e-6) if step.size else 1e-6
    out: list[dict[str, float | None]] = []
    for f in marks:
        at = f * RATE // FPS
        lo = max(0, at - RATE // 50)
        near = step[lo:at + RATE // 50]
        if not near.size:
            out.append({"at": f, "step": None, "offset_ms": None, "ratio": None})
            continue
        i = int(near.argmax())
        out.append({"at": f, "step": round(float(near[i]), 4), "offset_ms": round((lo + i - at) * 1000 / RATE, 2), "ratio": round(float(near[i]) / norm, 2)})
    return out


def lag(one, two) -> tuple[float | None, float | None]:  # type: ignore[no-untyped-def]
    import numpy as np

    m = min(len(one), len(two))
    if m < RATE // 10:
        return None, None
    x, y = one[:m] - one[:m].mean(), two[:m] - two[:m].mean()
    size = 1 << (2 * m - 1).bit_length()
    c = np.fft.irfft(np.fft.rfft(y, size) * np.conj(np.fft.rfft(x, size)), size)
    c = np.concatenate([c[-(m - 1):], c[:m]])
    return round((int(np.argmax(c)) - (m - 1)) * 1000 / RATE, 2), round(float(np.dot(x, y) / (np.linalg.norm(x) * np.linalg.norm(y) + 1e-12)), 4)


def strip(paths: list[str], at: int, out: Path) -> str:
    lo = max(0, at - 2)
    rows = "".join(f"[{i}:v]select='between(n,{lo},{lo + 4})',scale=320:-2,setpts=N/FRAME_RATE/TB,tile=5x1[r{i}];" for i in range(len(paths)))
    name = out / f"strip-{at}.png"
    stack = f"vstack=inputs={len(paths)}" if len(paths) > 1 else "null"
    run("ffmpeg", "-nostdin", "-v", "error", "-y", *[a for p in paths for a in ("-i", p)], "-filter_complex",
        rows + "".join(f"[r{i}]" for i in range(len(paths))) + stack, "-frames:v", "1", str(name))
    return name.name


def pair(path: str, at: int, out: Path) -> str:
    """Frames `at`-1 and `at` at full size, and their difference eight times over: a step across a join shows as a lit frame."""
    name = out / f"pair-{at}.png"
    run("ffmpeg", "-nostdin", "-v", "error", "-y", "-i", path, "-filter_complex",
        f"[0:v]select='between(n,{max(0, at - 1)},{at})',setpts=N/FRAME_RATE/TB,split=3[a][b][c];[a]trim=end_frame=1[first];"
        "[b]trim=start_frame=1,setpts=PTS-STARTPTS[second];[c]tblend=all_mode=difference,setpts=PTS-STARTPTS,"
        "lutyuv=y=val*8:u=128:v=128[step];[first][second][step]hstack=inputs=3", "-frames:v", "1", str(name))
    return name.name


def heard(paths: list[str], at: int, out: Path) -> str:
    """The second around frame `at` of each as a spectrogram, one over the next."""
    name = out / f"sound-{at}.png"
    start = max(0.0, at / FPS - 0.5)
    rows = "".join(f"[{i}:a]atrim=start={start}:duration=1,asetpts=PTS-STARTPTS,showspectrumpic=s=1000x240:legend=0:scale=log[r{i}];" for i in range(len(paths)))
    stack = f"vstack=inputs={len(paths)}" if len(paths) > 1 else "null"
    run("ffmpeg", "-nostdin", "-v", "error", "-y", *[a for p in paths for a in ("-i", p)], "-filter_complex",
        rows + "".join(f"[r{i}]" for i in range(len(paths))) + stack, "-frames:v", "1", str(name))
    return name.name


def drawn(make, *args: object) -> str:  # type: ignore[no-untyped-def]
    """A picture's name, or why it wasn't drawn: no picture ends the report."""
    try:
        return str(make(*args))
    except (subprocess.CalledProcessError, OSError) as error:
        said = error.stderr.decode(errors="replace")[-200:] if isinstance(error, subprocess.CalledProcessError) and error.stderr else str(error)
        return f"not drawn: {said.strip()}"


def pictures(before: str, after: str, whole: str, rest: list[str]) -> None:
    seams = [int(x) for x in rest[rest.index("--seams") + 1].split(",") if x]
    joins = [int(x) for x in rest[rest.index("--joins") + 1].split(",") if x]
    out = Path(rest[rest.index("--out") + 1])
    out.mkdir(parents=True, exist_ok=True)
    count = frames(after)
    # Segment 1 short of its last second: the windows there read segment 2's latents, which the arms' conditioning makes differ.
    first = range(0, max(1, seams[0] - FPS) if seams else count)
    later = range(seams[0] if seams else 0, count)
    psnr, ssim = compare(whole, after, out, "after-whole") if whole != "-" else ([], [])
    two = sound(after)
    report: dict[str, object] = {
        "frames": {"before": frames(before) if before != "-" else None, "after": count, "whole": frames(whole) if whole != "-" else None},
        "after_vs_whole": summary(psnr, ssim, seams + joins),
    }
    sounds: dict[str, object] = {"after": {"clicks": clicks(two, sorted(set(seams + joins))), "drift_ms": round((len(two) / RATE - count / FPS) * 1000, 1),
                                           "butts": butts(two, sorted(set(seams + joins)))}}
    moves: dict[str, object] = {"after": steps(after, seams), "after_joins": steps(after, joins)}
    # Each segment's brightness and its step at each seam: today's segments darken one after another (cr-288; an H3 edit's 67, 66,
    # 64, 60, 62 over a plate of 76).
    lit: dict[str, object] = {"after": brightness(after, seams), "after_joins": brightness(after, joins)["joins"]}
    report["brightness"] = lit
    if before != "-":
        lit["before"] = brightness(before, seams)
        psnr2, ssim2 = compare(before, after, out, "after-before")
        one = sound(before)
        report["after_vs_before"] = {"segment_1": summary(psnr2, ssim2, joins, first), "later": summary(psnr2, ssim2, seams + joins, later)}
        moves["before"] = steps(before, seams)
        sounds["before"] = {"clicks": clicks(one, seams), "drift_ms": round((len(one) / RATE - frames(before) / FPS) * 1000, 1), "butts": butts(one, seams)}
        sounds["segment_1"] = dict(zip(("lag_ms", "corr"), lag(one[: first.stop * RATE // FPS], two[: first.stop * RATE // FPS]), strict=True))
    report["steps"], report["sound"] = moves, sounds
    shown = [path for path in (before, after, whole) if path != "-"]
    voiced = [path for path in (before, after) if path != "-"]
    marks = sorted(set(seams + joins))
    report["strips"] = [drawn(strip, shown, at, out) for at in marks]
    report["pairs"] = [drawn(pair, after, at, out) for at in marks]
    report["sounds"] = [drawn(heard, voiced, at, out) for at in marks]
    print(json.dumps(report))


def main() -> None:
    verb, *rest = sys.argv[1:]
    if verb == "tap":
        tap(rest[0], rest[1], rest[rest.index("--stop") + 1])
    elif verb == "report":
        out = Path(rest[0])
        print(json.dumps({name: arm(out, name) for name in ("before", "after")}, indent=1))
    elif verb == "pictures":
        pictures(rest[0], rest[1], rest[2], rest[3:])
    else:
        raise SystemExit(f"incremental-video.py: no verb {verb}")


if __name__ == "__main__":
    main()
