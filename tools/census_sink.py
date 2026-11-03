#!/usr/bin/env python3
"""The census sink, recorder R1 (packet CEN-1, `docs/guides/census.md`).

In census mode the jail's netns has a default route (v4 and v6) into a TUN
device, `census0`, owned by the caller. Every packet the client sends to
anything but 10.137.0.0/30 lands there, and this process reads it off the
device's fd and writes one JSON line per packet. A TUN device has no wire:
with this reader attached the packets exist only as bytes here; with no
reader the kernel drops them.

**This file opens no socket.** It imports nothing from `socket`, `ssl`,
`http`, `urllib`, `subprocess` or `ctypes`, nor does `census_wire.py`,
which it uses (`test_census.py` pins both by AST and by the loaded module
set). Its only writes are the log file and, in `handshake` mode, replies
into the same TUN fd, which land back inside the netns.

  census_sink.py --dev census0 --out RUN/tun.jsonl [--mode observe|handshake]
  census_sink.py --fd 3 --out RUN/tun.jsonl          # an fd the helper attached
  census_sink.py --replay in.pcap --out x.jsonl [--mode handshake --replies out.pcap]

Modes (R-CENSUS §1.3): `observe` (the default, every verdict run) records
and never answers. `handshake` (control and diagnosis runs) answers a SYN
with a SYN-ACK, ACKs the first data segment then resets, and answers an
echo request, so the first data bytes (a TLS ClientHello, an HTTP request)
are recorded and `census_report.py` can name an IP-literal hit.

Log lines: `{"event": "start", ...}` first, then one line per packet
(`t` in Unix ms, `v`, `proto`, `src`, `dst`, ports or ICMP type/code, TCP
`flags`/`seq`, `len`, `plen`, `payload` as hex, at most 2048 bytes,
`replied`), and `{"event": "stop", ...}` on SIGTERM/SIGINT or the end of a
replay. A log with no stop line means the sink died, and the report calls
the run INVALID.
"""

import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.append(_HERE)  # last: tools/ holds a dis.py that shadows the stdlib's

import argparse  # noqa: E402
import fcntl  # noqa: E402
import json  # noqa: E402
import signal  # noqa: E402
import struct  # noqa: E402
import time  # noqa: E402

import census_wire as wire  # noqa: E402

#: `_IOW('T', 202, int)` from `<linux/if_tun.h>`.
TUNSETIFF = 0x400454CA
IFF_TUN = 0x0001
IFF_NO_PI = 0x1000

#: The census device's name (R-CENSUS §1.1). The sink attaches to nothing else.
DEVICE = "census0"

#: Payload bytes kept per packet (R-CENSUS §1.2, R1).
PAYLOAD_CAP = 2048

MODES = ("observe", "handshake")

#: pcap link types we can replay: raw IP, raw IPv4, raw IPv6, Ethernet.
LINKTYPE_RAW, LINKTYPE_IPV4, LINKTYPE_IPV6, LINKTYPE_ETHERNET = 101, 228, 229, 1


class Stop(Exception):
    """Raised by the signal handler to leave a blocking read."""


def now_ms() -> int:
    return int(time.time() * 1000)


def attach(dev: str = DEVICE) -> int:
    """Open `/dev/net/tun` and attach to the existing device `dev`.

    The helper (CEN-2) or the rootless holder (CEN-3) creates `dev` owned
    by the caller beforehand, so this needs no capability. Never called by
    the tests: nothing here creates a device on the build host.
    """
    if dev != DEVICE:
        raise SystemExit(f"census_sink: refusing device {dev!r}; only {DEVICE} is a census device")
    fd = os.open("/dev/net/tun", os.O_RDWR)
    ifr = struct.pack("16sH22x", dev.encode(), IFF_TUN | IFF_NO_PI)
    fcntl.ioctl(fd, TUNSETIFF, ifr)
    return fd


def record(pkt: bytes, t: int) -> dict:
    """The log line for one packet (without `replied`)."""
    try:
        p = wire.parse_packet(pkt)
    except wire.ParseError as err:
        return {"t": t, "bad": str(err), "len": len(pkt), "raw": pkt[:64].hex()}
    out = {"t": t}
    for k, v in p.items():
        if k.startswith("_") or k == "payload":
            continue
        out[k] = v
    payload = p.get("payload", b"")
    out["plen"] = len(payload)
    if payload:
        out["payload"] = payload[:PAYLOAD_CAP].hex()
    return out


def handle(pkt: bytes, mode: str, t: int):
    """`(log line, replies)` for one packet read off the device."""
    line = record(pkt, t)
    replies = []
    if mode == "handshake" and "bad" not in line:
        replies = wire.respond(wire.parse_packet(pkt))
    line["replied"] = len(replies)
    return line, replies


class Log:
    """Append-only JSON lines, opened by the sink itself (0600)."""

    def __init__(self, path: str):
        self.fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)

    def write(self, obj: dict) -> None:
        os.write(self.fd, (json.dumps(obj, sort_keys=True) + "\n").encode())

    def close(self) -> None:
        os.close(self.fd)


def run_tun(fd: int, log: Log, mode: str) -> str:
    """Read the device until a signal. Returns the stop reason."""
    def _stop(signum, _frame):
        raise Stop(signal.Signals(signum).name)

    signal.signal(signal.SIGTERM, _stop)
    signal.signal(signal.SIGINT, _stop)
    try:
        while True:
            pkt = os.read(fd, 65535)
            if not pkt:
                return "eof"
            line, replies = handle(pkt, mode, now_ms())
            log.write(line)
            for r in replies:
                os.write(fd, r)
    except Stop as s:
        return f"signal {s}"


# -------------------------------------------------------------------- pcap


def read_pcap(data: bytes):
    """`(ts_ms, packet)` pairs from a classic pcap of raw IP or Ethernet."""
    if len(data) < 24:
        raise ValueError("short pcap")
    magic = data[:4]
    if magic in (b"\xd4\xc3\xb2\xa1", b"\x4d\x3c\xb2\xa1"):
        end = "<"
    elif magic in (b"\xa1\xb2\xc3\xd4", b"\xa1\xb2\x3c\x4d"):
        end = ">"
    else:
        raise ValueError("not a pcap")
    nano = magic in (b"\x4d\x3c\xb2\xa1", b"\xa1\xb2\x3c\x4d")
    link = struct.unpack(end + "I", data[20:24])[0] & 0x0FFFFFFF
    if link not in (LINKTYPE_RAW, LINKTYPE_IPV4, LINKTYPE_IPV6, LINKTYPE_ETHERNET):
        raise ValueError(f"pcap link type {link} not supported")
    off = 24
    while off + 16 <= len(data):
        sec, frac, incl, _orig = struct.unpack(end + "IIII", data[off:off + 16])
        pkt = data[off + 16:off + 16 + incl]
        off += 16 + incl
        if link == LINKTYPE_ETHERNET:
            if len(pkt) < 14 or pkt[12:14] not in (b"\x08\x00", b"\x86\xdd"):
                continue
            pkt = pkt[14:]
        yield sec * 1000 + (frac // 1_000_000 if nano else frac // 1000), pkt


def write_pcap(packets) -> bytes:
    """A raw-IP pcap (link type 101) of `(ts_ms, packet)` pairs."""
    out = struct.pack("<IHHiIII", 0xA1B2C3D4, 2, 4, 0, 0, 65535, LINKTYPE_RAW)
    for ts, pkt in packets:
        out += struct.pack("<IIII", ts // 1000, (ts % 1000) * 1000, len(pkt), len(pkt)) + pkt
    return out


def run_replay(data: bytes, log: Log, mode: str):
    """Feed a pcap through the same handler. Returns the replies, stamped."""
    replies = []
    for ts, pkt in read_pcap(data):
        line, rs = handle(pkt, mode, ts)
        log.write(line)
        replies.extend((ts, r) for r in rs)
    return replies


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--dev", help=f"attach to this TUN device (only {DEVICE})")
    src.add_argument("--fd", type=int, help="read an already attached TUN fd")
    src.add_argument("--replay", help="read a pcap instead of a device")
    ap.add_argument("--out", required=True, help="the JSON-lines log (appended)")
    ap.add_argument("--mode", choices=MODES, default="observe")
    ap.add_argument("--replies", help="with --replay: write the replies as a pcap here")
    a = ap.parse_args(argv)

    log = Log(a.out)
    log.write({"event": "start", "t": now_ms(), "mode": a.mode, "pid": os.getpid(),
               "source": "replay" if a.replay else (a.dev or f"fd {a.fd}")})
    reason = "error"
    try:
        if a.replay:
            with open(a.replay, "rb") as f:
                replies = run_replay(f.read(), log, a.mode)
            if a.replies:
                with open(a.replies, "wb") as f:
                    f.write(write_pcap(replies))
            reason = "replay end"
        else:
            fd = a.fd if a.fd is not None else attach(a.dev)
            reason = run_tun(fd, log, a.mode)
    finally:
        log.write({"event": "stop", "t": now_ms(), "reason": reason})
        log.close()
    return 0 if reason != "error" else 1


if __name__ == "__main__":
    sys.exit(main())
