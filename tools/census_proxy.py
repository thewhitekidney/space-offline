#!/usr/bin/env python3
"""The dead proxy, recorder R4b (packet CEN-1, `docs/guides/census.md`).

The patcher's Tier A points the client's proxy env, Sentry DSN and resource
server at `127.0.0.1:9` (`tools/patches/<build>.json`). Normally nothing
listens there and every such request dies with ECONNREFUSED, unseen. In
census mode this listens there instead, reads the request head (at most
8 KiB, 5 s), writes one JSON line naming what was asked for, answers
`403 Forbidden` and closes. It never connects anywhere: it only accepts.

  census_proxy.py --out RUN/proxy.jsonl [--port 9]

It binds **only** `127.0.0.1` (`BIND_HOST`; there is no option to change
it, and `test_census.py` pins that). Inside the jail that is the netns's
own loopback. Port 9 is below 1024, so the netns needs
`net.ipv4.ip_unprivileged_port_start=9` (a per-netns sysctl) for the
caller to bind it: a CEN-2/CEN-3 hook, `docs/lanes/cen1.md`.

Log lines: `{"event": "start"|"stop", ...}`, and per connection `t`,
`peer`, `kind` (`connect`, `absolute`, `origin`, `tls`, `not-http`,
`empty`), `method`, `target`, `host`, `line` (the first line, at most
200 chars) or `raw` (hex, at most 64 bytes).
"""

import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.append(_HERE)

import argparse  # noqa: E402
import json  # noqa: E402
import signal  # noqa: E402
import socket  # noqa: E402
import threading  # noqa: E402
import time  # noqa: E402

import census_wire as wire  # noqa: E402

#: The only address this listener ever binds.
BIND_HOST = "127.0.0.1"
#: The discard port Tier A points at.
DEFAULT_PORT = 9
HEAD_CAP = 8192
READ_TIMEOUT = 5.0

RESPONSE = (b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n"
            b"X-Census: absorbed\r\n\r\n")


def classify(head: bytes) -> dict:
    """What one request head asked for."""
    if not head:
        return {"kind": "empty"}
    req = wire.http_request(head if b"\r\n" in head else head + b"\r\n")
    if req:
        method, target, host = req
        if method == "CONNECT":
            kind = "connect"
        elif "://" in target:
            kind = "absolute"
        else:
            kind = "origin"
        line = head.split(b"\r\n", 1)[0].decode("ascii", "replace")[:200]
        return {"kind": kind, "method": method, "target": target, "host": host, "line": line}
    sni = wire.tls_sni(head)
    if sni:
        return {"kind": "tls", "host": sni, "raw": head[:64].hex()}
    return {"kind": "not-http", "raw": head[:64].hex()}


def read_head(conn: socket.socket) -> bytes:
    conn.settimeout(READ_TIMEOUT)
    buf = b""
    try:
        while len(buf) < HEAD_CAP and b"\r\n\r\n" not in buf:
            chunk = conn.recv(HEAD_CAP - len(buf))
            if not chunk:
                break
            buf += chunk
            if buf[:1] == b"\x16" and len(buf) >= 5:
                need = 5 + int.from_bytes(buf[3:5], "big")
                if len(buf) >= min(need, HEAD_CAP):
                    break
    except OSError:
        pass
    return buf


class Proxy:
    def __init__(self, out_path: str, port: int = DEFAULT_PORT):
        self.lock = threading.Lock()
        self.fd = os.open(out_path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind((BIND_HOST, port))
        self.sock.listen(64)
        self.port = self.sock.getsockname()[1]

    def log(self, obj: dict) -> None:
        with self.lock:
            os.write(self.fd, (json.dumps(obj, sort_keys=True) + "\n").encode())

    def handle(self, conn: socket.socket, peer) -> None:
        try:
            t = int(time.time() * 1000)
            rec = {"t": t, "peer": f"{peer[0]}:{peer[1]}"}
            rec.update(classify(read_head(conn)))
            self.log(rec)
            try:
                conn.sendall(RESPONSE)
            except OSError:
                pass
        finally:
            conn.close()

    def serve(self, max_conns=None) -> None:
        n = 0
        while max_conns is None or n < max_conns:
            conn, peer = self.sock.accept()
            n += 1
            threading.Thread(target=self.handle, args=(conn, peer), daemon=True).start()

    def close(self) -> None:
        self.sock.close()
        os.close(self.fd)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", required=True)
    ap.add_argument("--port", type=int, default=DEFAULT_PORT)
    a = ap.parse_args(argv)
    p = Proxy(a.out, a.port)
    p.log({"event": "start", "t": int(time.time() * 1000), "bind": f"{BIND_HOST}:{p.port}",
           "pid": os.getpid()})

    def _stop(signum, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, _stop)
    reason = "error"
    try:
        p.serve()
    except KeyboardInterrupt:
        reason = "signal"
    finally:
        p.log({"event": "stop", "t": int(time.time() * 1000), "reason": reason})
        p.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
