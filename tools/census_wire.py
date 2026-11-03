"""Bytes on the wire for the census (packet CEN-1, `docs/guides/census.md`).

Pure functions only: parse and build IPv4/IPv6, TCP, UDP and ICMP packets,
the Internet checksum, a DNS question, a TLS ClientHello's SNI and an HTTP
request's Host. `census_sink.py` imports this module, so it is held to the
sink's rule: **no** `socket`, `ssl`, `http`, `urllib`, `subprocess` or
`ctypes`, here or in anything it imports (`test_census.py` pins both the
source and the loaded module set). Addresses are printed with `ipaddress`,
which needs none of those.
"""

import ipaddress
import struct
import zlib

PROTO_ICMP = 1
PROTO_TCP = 6
PROTO_UDP = 17
PROTO_ICMPV6 = 58

PROTO_NAMES = {PROTO_ICMP: "icmp", PROTO_TCP: "tcp", PROTO_UDP: "udp", PROTO_ICMPV6: "icmpv6"}

TCP_FIN = 0x01
TCP_SYN = 0x02
TCP_RST = 0x04
TCP_PSH = 0x08
TCP_ACK = 0x10
TCP_URG = 0x20

#: The MSS the handshake responder advertises (R-CENSUS §1.3).
HANDSHAKE_MSS = 1400

#: IPv6 extension headers we walk past to reach the transport header.
_V6_EXT = {0, 43, 60}
_V6_FRAG = 44
_V6_AH = 51
_V6_NONE = 59


class ParseError(ValueError):
    """The bytes are not a packet we can read."""


# ----------------------------------------------------------------- checksum


def checksum(data: bytes) -> int:
    """RFC 1071 Internet checksum of `data` (odd length padded with a zero)."""
    if len(data) % 2:
        data += b"\0"
    total = sum(struct.unpack(f"!{len(data) // 2}H", data))
    while total >> 16:
        total = (total & 0xFFFF) + (total >> 16)
    return ~total & 0xFFFF


def _pseudo(src: bytes, dst: bytes, proto: int, length: int) -> bytes:
    if len(src) == 4:
        return src + dst + struct.pack("!BBH", 0, proto, length)
    return src + dst + struct.pack("!I3xB", length, proto)


def l4_checksum(src: bytes, dst: bytes, proto: int, segment: bytes) -> int:
    """TCP, UDP or ICMPv6 checksum over the v4 or v6 pseudo-header."""
    return checksum(_pseudo(src, dst, proto, len(segment)) + segment)


def _addr(raw: bytes) -> str:
    return str(ipaddress.ip_address(raw))


def _raw(addr: str) -> bytes:
    return ipaddress.ip_address(addr).packed


# -------------------------------------------------------------------- parse


def tcp_flag_text(flags: int) -> str:
    """`SA`, `PA`, `R` ... in the order tcpdump prints them."""
    return "".join(c for bit, c in ((TCP_FIN, "F"), (TCP_SYN, "S"), (TCP_RST, "R"),
                                    (TCP_PSH, "P"), (TCP_ACK, "A"), (TCP_URG, "U"))
                   if flags & bit) or "."


def parse_packet(pkt: bytes) -> dict:
    """One raw IP packet (TUN with IFF_NO_PI) as a dict.

    Keys: `v`, `proto` (name or number), `src`, `dst`, `len`, and by
    transport: TCP `sport dport flags seq ack payload`, UDP `sport dport
    payload`, ICMP/ICMPv6 `type code payload`. `frag` is set for a non-first
    fragment, which carries no transport header. `_src_raw`, `_dst_raw` and
    `_l4` (the transport bytes) are for the responder; callers that log
    drop every key starting with `_`.
    """
    if not pkt:
        raise ParseError("empty")
    version = pkt[0] >> 4
    if version == 4:
        if len(pkt) < 20:
            raise ParseError("short v4 header")
        ihl = (pkt[0] & 0x0F) * 4
        total = struct.unpack("!H", pkt[2:4])[0]
        if ihl < 20 or total < ihl or len(pkt) < total:
            raise ParseError("bad v4 lengths")
        frag_off = struct.unpack("!H", pkt[6:8])[0] & 0x1FFF
        proto = pkt[9]
        src_raw, dst_raw = pkt[12:16], pkt[16:20]
        l4 = pkt[ihl:total]
        out = {"v": 4, "len": total}
        if frag_off:
            out["frag"] = True
    elif version == 6:
        if len(pkt) < 40:
            raise ParseError("short v6 header")
        plen = struct.unpack("!H", pkt[4:6])[0]
        if len(pkt) < 40 + plen:
            raise ParseError("bad v6 length")
        proto = pkt[6]
        src_raw, dst_raw = pkt[8:24], pkt[24:40]
        off, end = 40, 40 + plen
        out = {"v": 6, "len": end}
        while proto in _V6_EXT or proto in (_V6_FRAG, _V6_AH):
            if off + 8 > end:
                raise ParseError("short v6 extension header")
            nxt = pkt[off]
            if proto == _V6_FRAG:
                if struct.unpack("!H", pkt[off + 2:off + 4])[0] >> 3:
                    out["frag"] = True
                hlen = 8
            elif proto == _V6_AH:
                hlen = (pkt[off + 1] + 2) * 4
            else:
                hlen = (pkt[off + 1] + 1) * 8
            proto, off = nxt, off + hlen
        l4 = pkt[off:end]
    else:
        raise ParseError(f"ip version {version}")
    out.update(proto=PROTO_NAMES.get(proto, proto), src=_addr(src_raw), dst=_addr(dst_raw),
               _src_raw=src_raw, _dst_raw=dst_raw, _l4=l4)
    if out.get("frag") or proto == _V6_NONE:
        return out
    if proto == PROTO_TCP:
        if len(l4) < 20:
            raise ParseError("short tcp header")
        sport, dport, seq, ack, offf, flags = struct.unpack("!HHIIBB", l4[:14])
        doff = (offf >> 4) * 4
        if doff < 20 or doff > len(l4):
            raise ParseError("bad tcp data offset")
        out.update(sport=sport, dport=dport, seq=seq, ack=ack, flags=tcp_flag_text(flags),
                   _flags=flags, payload=l4[doff:])
    elif proto == PROTO_UDP:
        if len(l4) < 8:
            raise ParseError("short udp header")
        sport, dport = struct.unpack("!HH", l4[:4])
        out.update(sport=sport, dport=dport, payload=l4[8:])
    elif proto in (PROTO_ICMP, PROTO_ICMPV6):
        if len(l4) < 4:
            raise ParseError("short icmp header")
        out.update(type=l4[0], code=l4[1], payload=l4[4:])
    return out


def verify_checksums(pkt: bytes) -> bool:
    """True when the IPv4 header and the transport checksum both verify."""
    p = parse_packet(pkt)
    if p["v"] == 4 and checksum(pkt[:(pkt[0] & 0x0F) * 4]) != 0:
        return False
    proto = {"tcp": PROTO_TCP, "udp": PROTO_UDP, "icmpv6": PROTO_ICMPV6}.get(p["proto"])
    if proto is not None:
        return l4_checksum(p["_src_raw"], p["_dst_raw"], proto, p["_l4"]) == 0
    if p["proto"] == "icmp":
        return checksum(p["_l4"]) == 0
    return True


# -------------------------------------------------------------------- build


def build_ipv4(src: str, dst: str, proto: int, payload: bytes, ttl: int = 64, ident: int = 0) -> bytes:
    hdr = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 20 + len(payload), ident, 0x4000, ttl, proto, 0,
                      _raw(src), _raw(dst))
    hdr = hdr[:10] + struct.pack("!H", checksum(hdr)) + hdr[12:]
    return hdr + payload


def build_ipv6(src: str, dst: str, nh: int, payload: bytes, hlim: int = 64) -> bytes:
    return struct.pack("!IHBB16s16s", 6 << 28, len(payload), nh, hlim, _raw(src), _raw(dst)) + payload


def build_ip(src: str, dst: str, proto: int, payload: bytes) -> bytes:
    """v4 or v6 by the address family of `src`."""
    if ipaddress.ip_address(src).version == 4:
        return build_ipv4(src, dst, proto, payload)
    return build_ipv6(src, dst, proto, payload)


def build_tcp(src: str, dst: str, sport: int, dport: int, seq: int, ack: int, flags: int,
              payload: bytes = b"", window: int = 65535, options: bytes = b"") -> bytes:
    """A whole IP packet carrying one TCP segment, checksums filled in."""
    if len(options) % 4:
        options += b"\0" * (4 - len(options) % 4)
    doff = (20 + len(options)) // 4
    seg = struct.pack("!HHIIBBHHH", sport, dport, seq & 0xFFFFFFFF, ack & 0xFFFFFFFF,
                      doff << 4, flags, window, 0, 0) + options + payload
    csum = l4_checksum(_raw(src), _raw(dst), PROTO_TCP, seg)
    seg = seg[:16] + struct.pack("!H", csum) + seg[18:]
    return build_ip(src, dst, PROTO_TCP, seg)


def build_udp(src: str, dst: str, sport: int, dport: int, payload: bytes) -> bytes:
    seg = struct.pack("!HHHH", sport, dport, 8 + len(payload), 0) + payload
    csum = l4_checksum(_raw(src), _raw(dst), PROTO_UDP, seg) or 0xFFFF
    seg = seg[:6] + struct.pack("!H", csum) + seg[8:]
    return build_ip(src, dst, PROTO_UDP, seg)


def build_icmp(src: str, dst: str, icmp_type: int, code: int, rest: bytes) -> bytes:
    """ICMP (v4 source) or ICMPv6 (v6 source); `rest` follows type/code/csum."""
    msg = struct.pack("!BBH", icmp_type, code, 0) + rest
    if ipaddress.ip_address(src).version == 4:
        csum = checksum(msg)
        return build_ipv4(src, dst, PROTO_ICMP, msg[:2] + struct.pack("!H", csum) + msg[4:])
    csum = l4_checksum(_raw(src), _raw(dst), PROTO_ICMPV6, msg)
    return build_ipv6(src, dst, PROTO_ICMPV6, msg[:2] + struct.pack("!H", csum) + msg[4:])


# ---------------------------------------------------------------- responder


def isn_for(src: str, dst: str, sport: int, dport: int) -> int:
    """The responder's initial sequence number: a pure function of the flow,
    so no state is kept between packets."""
    return zlib.crc32(f"{src}|{dst}|{sport}|{dport}".encode()) & 0xFFFFFFFF


def respond(p: dict) -> list:
    """The handshake-mode replies to one parsed packet (R-CENSUS §1.3).

    SYN -> SYN-ACK with MSS 1400. A segment with data -> ACK it, then RST.
    FIN -> RST. ICMP/ICMPv6 echo request -> echo reply. Everything else,
    including all UDP, gets nothing. Replies swap source and destination,
    so they land back inside the jail's netns.
    """
    proto = p.get("proto")
    if p.get("frag"):
        return []
    if proto == "tcp":
        flags = p["_flags"]
        if flags & TCP_RST:
            return []
        me, peer = p["dst"], p["src"]
        isn = isn_for(p["src"], p["dst"], p["sport"], p["dport"])
        if flags & TCP_SYN and not flags & TCP_ACK:
            mss = struct.pack("!BBH", 2, 4, HANDSHAKE_MSS)
            return [build_tcp(me, peer, p["dport"], p["sport"], isn, p["seq"] + 1,
                              TCP_SYN | TCP_ACK, options=mss)]
        n = len(p["payload"])
        if n:
            ack = p["seq"] + n + (1 if flags & TCP_FIN else 0)
            return [build_tcp(me, peer, p["dport"], p["sport"], isn + 1, ack, TCP_ACK),
                    build_tcp(me, peer, p["dport"], p["sport"], isn + 1, 0, TCP_RST)]
        if flags & TCP_FIN:
            return [build_tcp(me, peer, p["dport"], p["sport"], isn + 1, p["seq"] + 1, TCP_RST | TCP_ACK)]
        return []
    if proto == "icmp" and p["type"] == 8:
        return [build_icmp(p["dst"], p["src"], 0, 0, p["_l4"][4:])]
    if proto == "icmpv6" and p["type"] == 128:
        return [build_icmp(p["dst"], p["src"], 129, 0, p["_l4"][4:])]
    return []


# ---------------------------------------------------------- names in payload


def dns_query(name: str, qtype: int = 1, ident: int = 0x4345) -> bytes:
    """A standard recursive query for one name."""
    out = struct.pack("!HHHHHH", ident, 0x0100, 1, 0, 0, 0)
    for label in name.rstrip(".").split("."):
        raw = label.encode("ascii")
        if not 0 < len(raw) < 64:
            raise ValueError(f"bad label in {name!r}")
        out += bytes([len(raw)]) + raw
    return out + b"\0" + struct.pack("!HH", qtype, 1)


def dns_qname(msg: bytes):
    """The first question's name of a DNS message, lower case, no trailing
    dot; None when it is not a query we can read (no compression in a
    question, which a sane client never sends)."""
    if len(msg) < 12 or struct.unpack("!H", msg[4:6])[0] < 1:
        return None
    off, labels = 12, []
    while off < len(msg):
        n = msg[off]
        if n == 0:
            return ".".join(labels).lower() if labels else None
        if n & 0xC0 or off + 1 + n > len(msg):
            return None
        labels.append(msg[off + 1:off + 1 + n].decode("ascii", "replace"))
        off += 1 + n
    return None


def tls_sni(data: bytes):
    """The server_name of a TLS ClientHello, or None. Reads one record."""
    try:
        if len(data) < 5 or data[0] != 22:
            return None
        rec = data[5:5 + struct.unpack("!H", data[3:5])[0]]
        if not rec or rec[0] != 1:
            return None
        body = rec[4:4 + int.from_bytes(rec[1:4], "big")]
        off = 2 + 32
        off += 1 + body[off]
        off += 2 + struct.unpack("!H", body[off:off + 2])[0]
        off += 1 + body[off]
        end = off + 2 + struct.unpack("!H", body[off:off + 2])[0]
        off += 2
        while off + 4 <= end:
            etype, elen = struct.unpack("!HH", body[off:off + 4])
            ext = body[off + 4:off + 4 + elen]
            if etype == 0:
                pos = 2
                while pos + 3 <= len(ext):
                    ntype, nlen = ext[pos], struct.unpack("!H", ext[pos + 1:pos + 3])[0]
                    if ntype == 0:
                        return ext[pos + 3:pos + 3 + nlen].decode("ascii", "replace").lower()
                    pos += 3 + nlen
                return None
            off += 4 + elen
    except (IndexError, struct.error):
        return None
    return None


HTTP_METHODS = (b"GET", b"POST", b"PUT", b"HEAD", b"DELETE", b"OPTIONS", b"PATCH", b"CONNECT")


def http_request(data: bytes):
    """`(method, target, host)` of an HTTP/1 request head, or None.

    `host` is the Host header, else the authority of an absolute target or
    a CONNECT target; lower case, port kept."""
    line, _, rest = data.partition(b"\r\n")
    parts = line.split(b" ")
    if len(parts) != 3 or parts[0] not in HTTP_METHODS or not parts[2].startswith(b"HTTP/"):
        return None
    method, target = parts[0].decode(), parts[1].decode("ascii", "replace")
    host = None
    for hdr in rest.split(b"\r\n"):
        if not hdr:
            break
        k, _, v = hdr.partition(b":")
        if k.strip().lower() == b"host":
            host = v.strip().decode("ascii", "replace").lower()
            break
    if host is None:
        if method == "CONNECT":
            host = target.lower()
        elif "://" in target:
            host = target.split("://", 1)[1].split("/", 1)[0].lower()
    return method, target, host


def payload_name(payload: bytes, dport=None):
    """The name a payload asks for: SNI, HTTP Host, or a DNS qname (port 53)."""
    if dport == 53:
        return dns_qname(payload)
    sni = tls_sni(payload)
    if sni:
        return sni
    req = http_request(payload)
    return req[2] if req else None
