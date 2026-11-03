#!/usr/bin/env python3
"""Forward jail-side TCP to a loopback listener, without opening the jail.

Why this is needed, and why the obvious alternative is wrong.

The jail's third layer (`docs/guides/jail.md`) drops forwarding for the jail subnet
and the veth outright:

    chain forward { iifname "eve-host" drop; oifname "eve-host" drop; ... }

That is deliberate -- the host has ip_forward=1 because Docker and libvirt are
installed, so without it the namespace could route *out through the host*.

Our own server is a host process listening on 10.137.0.1, so its traffic
terminates in INPUT and never meets that chain. A server in a **container**
does not: publishing a port means DNAT to the bridge subnet, which is a routing
decision, which is FORWARD, which the jail drops. The client's connection dies
with no log line anywhere useful.

The tempting fix is to poke a hole in the forward chain. Don't: that weakens
the one layer that stops the namespace routing anywhere at all, and it does so
permanently, for a test.

Instead this terminates the connection in userspace on the host -- INPUT, like
our own server -- and makes a *fresh* loopback connection to the container's
published port. The forward chain is never involved and the jail is untouched.

    tools/jail-forward.py 26000 26001 26002
    tools/jail-forward.py --capture captures/evejs 26000

Binds 10.137.0.1:PORT for each PORT and forwards to 127.0.0.1:PORT. No root:
every port used here is above 1024.

## The tap

With --capture DIR, every frame on port 26000 is also written to a `.evecap`
in DIR, in the format `eve-server`'s own `capture.rs` writes -- so
`evecap FILE --tags` decodes a third-party server's conversation with our
client using *our* marshal, with no second decoder to keep correct.

That is the point of doing it here. This process already sits between the two
(see above), so the tap costs nothing extra, needs no root, and needs no pcap:
we record **frames**, the boundaries the framing layer decided on, exactly as
our server does. A disagreement between capture and client behaviour then
localises to the splitter rather than to a TCP reassembler.

Framing is `docs/reference/framing.md`: word0 is u32 LE with plen in bits 0..27 (the
bytes that FOLLOW word0), bit 28 OOB_PRESENT, bit 29 ZLIB. An OOB frame
carries `u32 oobLen` plus that many bytes before the payload, so the payload
starts at `4 + (oob ? 4 + oobLen : 0)`. We record the payload with word0 and
any OOB prefix stripped, which is what `push.rs` hands `Capture::write`.
"""

# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import os
import socket
import struct
import sys
import threading
import time

LISTEN_IP = os.environ.get("JAIL_FORWARD_LISTEN", "10.137.0.1")
TARGET_IP = os.environ.get("JAIL_FORWARD_TARGET", "127.0.0.1")
BUFSIZ = 65536

# capture.rs: magic, version, then (dir u8, micros u64 LE, len u32 LE, bytes).
CAP_MAGIC = b"EVEOFFLINECAP\n"
CAP_VERSION = 1
DIR_CLIENT_TO_SERVER = 0
DIR_SERVER_TO_CLIENT = 1

# The client's own SocketGPS.MaxPacketSize. A plen above it means we have lost
# frame sync, and the only honest response is to stop recording rather than
# write garbage into an artefact whose whole value is that it is trustworthy.
MAX_FRAME = 10_485_760

PLEN_MASK = 0x0FFF_FFFF
FLAG_OOB = 0x1000_0000
FLAG_ZLIB = 0x2000_0000


class Capture:
    """An append-only .evecap, shared by both directions of one connection.

    One lock for both directions, for the reason push.rs gives: the capture
    then has a single serialisation point and its record order is the socket's
    order. It costs one uncontended lock per frame.
    """

    def __init__(self, path):
        self.path = path
        self.lock = threading.Lock()
        self.opened = time.monotonic()
        self.frames = 0
        self.oob_frames = 0
        self.zlib_frames = 0
        self.fh = open(path, "wb")
        self.fh.write(CAP_MAGIC + bytes([CAP_VERSION]))
        self.fh.flush()

    def write(self, direction, payload, oob=False, zlib=False):
        micros = int((time.monotonic() - self.opened) * 1_000_000)
        rec = bytes([direction]) + struct.pack("<QI", micros, len(payload)) + payload
        with self.lock:
            self.frames += 1
            if oob:
                self.oob_frames += 1
            if zlib:
                self.zlib_frames += 1
            try:
                self.fh.write(rec)
                self.fh.flush()
            except OSError as exc:
                print(f"[capture] write failed, recording stops: {exc}", flush=True)
                self.fh = None

    def close(self):
        with self.lock:
            if self.fh is not None:
                try:
                    self.fh.close()
                except OSError:
                    pass
                self.fh = None
        extra = []
        if self.oob_frames:
            extra.append(f"{self.oob_frames} oob")
        if self.zlib_frames:
            extra.append(f"{self.zlib_frames} zlib")
        tail = f" ({', '.join(extra)})" if extra else ""
        print(f"[capture] {self.path}: {self.frames} frames{tail}", flush=True)


class Splitter:
    """Byte stream in, whole frames out, one direction.

    Deliberately strict: a plen over MAX_FRAME is treated as lost sync and
    disables the tap for this direction instead of guessing. Forwarding is
    never affected -- the bytes go through untouched whatever this decides.
    """

    def __init__(self, tag):
        self.buf = bytearray()
        self.tag = tag
        self.live = True

    def feed(self, data):
        """Return a list of (payload, oob, zlib) for every frame now complete.

        A list, not a generator: as a generator the `del self.buf[:total]`
        after each yield would only run if the caller fully consumed it, so a
        caller that broke early would silently re-emit the same frame forever.
        """
        out = []
        if not self.live:
            return out
        self.buf += data
        while len(self.buf) >= 4:
            word = struct.unpack_from("<I", self.buf, 0)[0]
            plen = word & PLEN_MASK
            if plen > MAX_FRAME:
                print(
                    f"[capture] {self.tag}: plen {plen} over MAX_FRAME; "
                    "frame sync lost, tap off for this direction",
                    flush=True,
                )
                self.live = False
                self.buf.clear()
                return out
            total = 4 + plen
            if len(self.buf) < total:
                return out
            oob = bool(word & FLAG_OOB)
            start = 4
            if oob:
                if plen < 4:
                    print(f"[capture] {self.tag}: oob frame shorter than its own length word", flush=True)
                    self.live = False
                    self.buf.clear()
                    return out
                oob_len = struct.unpack_from("<I", self.buf, 4)[0]
                start = 8 + oob_len
                if start > total:
                    print(f"[capture] {self.tag}: oobLen {oob_len} runs past the frame", flush=True)
                    self.live = False
                    self.buf.clear()
                    return out
            out.append((bytes(self.buf[start:total]), oob, bool(word & FLAG_ZLIB)))
            del self.buf[:total]
        return out


def pump(src, dst, tag, cap=None, direction=None, splitter=None):
    """Copy until EOF, then half-close so the peer sees the end of stream.

    The tap is strictly observational: `dst.sendall(data)` happens first and
    with the bytes untouched, so nothing the splitter or the capture decides
    can change what the peer receives. A capture failure must degrade to a
    working forwarder, never the reverse.
    """
    try:
        while True:
            data = src.recv(BUFSIZ)
            if not data:
                break
            dst.sendall(data)
            if cap is not None and splitter is not None:
                try:
                    for payload, oob, zlib in splitter.feed(data):
                        cap.write(direction, payload, oob=oob, zlib=zlib)
                except Exception as exc:  # never let the tap break the proxy
                    print(f"[capture] {tag}: tap off: {exc!r}", flush=True)
                    splitter.live = False
    except OSError:
        pass
    finally:
        # A half-close, not a close: the other direction may still be live.
        try:
            dst.shutdown(socket.SHUT_WR)
        except OSError:
            pass


def handle(client, peer, port, cap_dir=None, conn_id=0):
    print(f"[{port}] open  {peer[0]}:{peer[1]}", flush=True)
    try:
        upstream = socket.create_connection((TARGET_IP, port), timeout=10)
    except OSError as exc:
        print(f"[{port}] upstream {TARGET_IP}:{port} unreachable: {exc}", flush=True)
        client.close()
        return
    # create_connection's timeout is a CONNECT timeout only in intent, but it
    # leaves the socket in timeout mode, so it then applies to every recv() as
    # an IDLE timeout too. That killed two real sessions exactly 10s after the
    # last packet: recv() raised socket.timeout, the OSError handler in pump()
    # swallowed it, and the connection was torn down mid-login.
    #
    # Ten seconds is never right for this protocol. The client's own
    # keep-alive is 240s (clientKeepAliveTimerInterval, see
    # docs/archive/destiny-and-the-ballpark.md section 5.3, which also says you cannot
    # ride inbound frames to infer liveness). Long silences are NORMAL here --
    # the client goes quiet waiting to be told things. Blocking is correct: a
    # dead peer surfaces as EOF or ECONNRESET, which pump() already handles.
    upstream.settimeout(None)
    client.settimeout(None)
    # Nagle off on both legs: this carries an interactive game protocol whose
    # frames are small and latency-sensitive.
    for sock in (client, upstream):
        try:
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except OSError:
            pass
    cap = None
    if cap_dir is not None:
        # Named as eve-server names its own: <unix secs>-<pid>-<id:04>.evecap,
        # so both sets sort together and tooling needs no special case.
        name = f"{int(time.time())}-{os.getpid()}-{conn_id:04d}.evecap"
        try:
            cap = Capture(os.path.join(cap_dir, name))
            print(f"[{port}] capturing -> {cap.path}", flush=True)
        except OSError as exc:
            print(f"[{port}] capture unavailable ({exc}); forwarding anyway", flush=True)

    threads = [
        threading.Thread(
            target=pump,
            args=(client, upstream, f"{port}/up", cap, DIR_CLIENT_TO_SERVER,
                  Splitter(f"{port}/up") if cap else None),
            daemon=True,
        ),
        threading.Thread(
            target=pump,
            args=(upstream, client, f"{port}/down", cap, DIR_SERVER_TO_CLIENT,
                  Splitter(f"{port}/down") if cap else None),
            daemon=True,
        ),
    ]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    client.close()
    upstream.close()
    if cap is not None:
        cap.close()
    print(f"[{port}] close {peer[0]}:{peer[1]}", flush=True)


def listener(port, cap_dir=None):
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        srv.bind((LISTEN_IP, port))
    except OSError as exc:
        print(f"[{port}] cannot bind {LISTEN_IP}:{port}: {exc}", file=sys.stderr, flush=True)
        return
    srv.listen(64)
    tap = f"  (capturing to {cap_dir})" if cap_dir else ""
    print(f"[{port}] {LISTEN_IP}:{port} -> {TARGET_IP}:{port}{tap}", flush=True)
    conn_id = 0
    while True:
        try:
            client, peer = srv.accept()
        except OSError:
            break
        threading.Thread(
            target=handle, args=(client, peer, port, cap_dir, conn_id), daemon=True
        ).start()
        conn_id += 1


def main(argv):
    args = argv[1:]
    cap_dir = None
    if "--capture" in args:
        i = args.index("--capture")
        try:
            cap_dir = args[i + 1]
        except IndexError:
            print("--capture needs a directory", file=sys.stderr)
            return 2
        del args[i : i + 2]
        try:
            os.makedirs(cap_dir, exist_ok=True)
        except OSError as exc:
            print(f"cannot create {cap_dir}: {exc}", file=sys.stderr)
            return 2
    ports = [int(a) for a in args] or [26000]
    threads = [
        threading.Thread(target=listener, args=(p, cap_dir), daemon=True) for p in ports
    ]
    for t in threads:
        t.start()
    try:
        for t in threads:
            t.join()
    except KeyboardInterrupt:
        print("\nstopped", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
