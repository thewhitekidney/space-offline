#!/usr/bin/env python3
"""Prepare a *copy* of an EVE client to talk to an eve-offline server.

The one change the client needs is `cryptoPack = Placebo` in `start.ini`
(see docs/reference/login-path.md section 3): under the stock `CryptoAPI` pack every
packet after the handshake is encrypted against CCP's key, which a private
server does not have, and the handshake expects a signed response we cannot
produce. Placebo makes the crypto layer the identity, so the stream is plain.

The snag is that `start.ini` is covered by `manifest.dat`, which stores a
SHA-256 of each listed file. Editing `start.ini` makes its hash no longer
match, and the client shows

    Verification Failure
    Your EVE client installation may have modified, damaged or corrupt files.
    VerifyManifestFile: ...

This tool does NOT disable or bypass that check. It updates the manifest to
tell the truth about the file the owner changed: it recomputes the SHA-256 of
the edited `start.ini` and writes it back into the one manifest entry for
`app:\\start.ini`. Nothing else in the manifest is touched, and no binary is
modified. Conceptually it is re-signing a checksum to match a change you made
to your own property.

Whether that is SUFFICIENT depends on something this tool cannot settle on its
own: whether `manifest.dat` is itself integrity-checked (a signature over the
whole manifest). If it is not, the client accepts the updated manifest and this
is all that is needed. If it is, the client will still refuse, and a different
approach is required. The honest test is to launch the client afterwards; this
tool reports that this question is open rather than pretending it is closed.

Everything is reversible: `start.ini` and `manifest.dat` are backed up before
the first change, and `--restore` puts them back byte for byte.

Scope and safety:
  * Operates on a COPY of a client you own, never the install you play online.
    It refuses a directory named `tq` (the usual live-install name) unless
    --allow-real is given, because a modified live install fails Steam's own
    validation and, if the jail ever leaked, would be the client sitting next
    to your real account state.
  * Pure standard library. Nothing is installed. Run it with `-I`.
  * It changes exactly two files and verifies its own work; on any failure it
    restores from the backups it just made rather than leaving a half-edited
    client.

Usage:
    tools/client-prepare.py <client-dir> [--pack PACK] [--allow-real]
    tools/client-prepare.py <client-dir> --check
    tools/client-prepare.py <client-dir> --restore

`<client-dir>` is the directory containing `start.ini`, `manifest.dat` and
`bin64/` (e.g. .../CCP/EVE/tq-offline).
"""

# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse
import hashlib
import struct
import sys
from pathlib import Path

# The manifest entry whose hash we recompute. This is the EVE virtual path,
# exactly as it appears in manifest.dat (a Windows-style backslash path), not
# the host filesystem path.
MANIFEST_ENTRY = b"app:\\start.ini"

# The file on disk that entry refers to.
CLIENT_FILE = "start.ini"

BACKUP_SUFFIX = ".eveoffline-orig"


def locate_hash(manifest: bytes, entry: bytes):
    """Find (offset, length) of the stored hash for `entry` in the manifest.

    The record layout, confirmed by reading manifest.dat: a u32 name length,
    the name bytes, a u32 hash length, then the hash. We search for the
    length-prefixed name so we match a record boundary and not an incidental
    occurrence of the string, and we require the name to appear exactly once so
    the location is unambiguous. We do not parse the rest of the file: the tail
    uses an irregular layout we deliberately leave untouched.
    """
    needle = struct.pack("<I", len(entry)) + entry
    first = manifest.find(needle)
    if first < 0:
        raise LookupError(f"{entry!r} not found in manifest")
    if manifest.find(needle, first + 1) >= 0:
        raise LookupError(f"{entry!r} appears more than once; refusing to guess")
    after_name = first + len(needle)
    (hash_len,) = struct.unpack_from("<I", manifest, after_name)
    if hash_len != 32:
        raise LookupError(f"expected a 32-byte SHA-256, manifest says {hash_len}")
    hash_off = after_name + 4
    if hash_off + hash_len > len(manifest):
        raise LookupError("manifest truncated before the hash")
    return hash_off, hash_len


def set_crypto_pack(text: str, pack: str) -> str:
    """Return `start.ini` text with the cryptoPack line set to `pack`.

    Preserves every other byte, including line endings, so the only change to
    the file is the one value — which keeps the diff, and the reason for the
    new hash, trivial to see.
    """
    out = []
    changed = False
    for line in text.splitlines(keepends=True):
        stripped = line.strip()
        if stripped.lower().startswith("cryptopack"):
            eol = line[len(line.rstrip("\r\n")) :]
            out.append(f"cryptoPack = {pack}{eol}")
            changed = True
        else:
            out.append(line)
    if not changed:
        raise ValueError("no cryptoPack line in start.ini")
    return "".join(out)


def read_pack(text: str) -> str:
    for line in text.splitlines():
        s = line.strip()
        if s.lower().startswith("cryptopack"):
            return s.split("=", 1)[1].strip()
    return "<unset>"


def backup(path: Path):
    bak = path.with_name(path.name + BACKUP_SUFFIX)
    if not bak.exists():
        bak.write_bytes(path.read_bytes())
    return bak


def do_check(client: Path) -> int:
    ini = client / CLIENT_FILE
    man = client / "manifest.dat"
    text = ini.read_text()
    pack = read_pack(text)
    manifest = man.read_bytes()
    hoff, _ = locate_hash(manifest, MANIFEST_ENTRY)
    stored = manifest[hoff : hoff + 32]
    actual = hashlib.sha256(ini.read_bytes()).digest()
    matches = stored == actual
    print(f"client:            {client}")
    print(f"cryptoPack:        {pack}")
    print(f"manifest hash:     {stored.hex()}")
    print(f"sha256(start.ini): {actual.hex()}")
    print(f"manifest matches:  {matches}")
    has_backup = (ini.with_name(ini.name + BACKUP_SUFFIX)).exists()
    print(f"backups present:   {has_backup}")
    if pack == "Placebo" and matches:
        print("\nready: cryptoPack is Placebo and the manifest agrees with the file.")
    elif pack != "Placebo":
        print("\nnot prepared: cryptoPack is not Placebo (run without --check to set it).")
    else:
        print("\ninconsistent: Placebo is set but the manifest hash does not match.")
    return 0


def do_restore(client: Path) -> int:
    restored = []
    for name in (CLIENT_FILE, "manifest.dat"):
        path = client / name
        bak = path.with_name(path.name + BACKUP_SUFFIX)
        if bak.exists():
            path.write_bytes(bak.read_bytes())
            restored.append(name)
    if not restored:
        print("nothing to restore: no backups found.", file=sys.stderr)
        return 1
    print(f"restored from backup: {', '.join(restored)}")
    print("the client is back to its original, manifest-consistent state.")
    return 0


def do_prepare(client: Path, pack: str, rehash: bool) -> int:
    ini = client / CLIENT_FILE
    man = client / "manifest.dat"

    # Locate the hash before writing anything, so a manifest we do not
    # understand stops us up front rather than after a half-done edit.
    manifest = bytearray(man.read_bytes())
    hoff, _ = locate_hash(manifest, MANIFEST_ENTRY)

    ini_backup = backup(ini)
    man_backup = backup(man)

    # 1. Edit start.ini.
    new_text = set_crypto_pack(ini.read_text(), pack)
    ini.write_text(new_text, newline="")

    # 2. Rewrite the manifest hash to match the new file -- unless --no-rehash,
    #    which leaves manifest.dat stock. That is a diagnostic: it tells us
    #    whether the manifest's start.ini hash is actually enforced at boot, or
    #    whether editing the manifest is what the client objects to.
    new_hash = hashlib.sha256(ini.read_bytes()).digest()
    if rehash:
        manifest[hoff : hoff + 32] = new_hash
        man.write_bytes(manifest)

    # 3. Verify our own work by re-reading from disk. If anything is off, undo
    #    both files from the backups we just made and fail loudly.
    check = bytearray(man.read_bytes())
    pack_ok = read_pack(ini.read_text()) == pack
    man_ok = (not rehash) or check[hoff : hoff + 32] == hashlib.sha256(ini.read_bytes()).digest()
    if not (pack_ok and man_ok):
        ini.write_bytes(ini_backup.read_bytes())
        man.write_bytes(man_backup.read_bytes())
        print("verification of the edit failed; restored from backup.", file=sys.stderr)
        return 1

    print(f"prepared {client}")
    print(f"  cryptoPack       -> {pack}")
    print(f"  start.ini sha256 -> {new_hash.hex()}")
    if rehash:
        print(f"  manifest entry for {MANIFEST_ENTRY.decode('latin1')} updated to match")
    else:
        print("  manifest.dat LEFT STOCK (--no-rehash): its start.ini entry now")
        print("  disagrees with the file on purpose. This is the diagnostic run.")
    print(f"  backups: {ini_backup.name}, {man_backup.name}  (undo with --restore)")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Prepare a copy of an EVE client for an eve-offline server.",
        epilog="Operates on a COPY you own, never the install you play online.",
    )
    ap.add_argument("client", type=Path, help="client directory (contains start.ini)")
    ap.add_argument("--pack", default="Placebo", help="cryptoPack value (default: Placebo)")
    ap.add_argument(
        "--no-rehash",
        action="store_true",
        help="edit start.ini but leave manifest.dat stock (diagnostic)",
    )
    ap.add_argument("--check", action="store_true", help="report state, change nothing")
    ap.add_argument("--restore", action="store_true", help="undo from backups")
    ap.add_argument(
        "--allow-real",
        action="store_true",
        help="permit operating on a directory named 'tq' (the live install)",
    )
    args = ap.parse_args()

    client = args.client
    if not (client / CLIENT_FILE).is_file():
        print(f"no {CLIENT_FILE} in {client}", file=sys.stderr)
        return 2
    if not (client / "manifest.dat").is_file():
        print(f"no manifest.dat in {client}", file=sys.stderr)
        return 2
    if not (client / "bin64" / "exefile.exe").is_file():
        print(f"{client} does not look like an EVE client (no bin64/exefile.exe)", file=sys.stderr)
        return 2
    if client.name == "tq" and not args.allow_real:
        print(
            f"{client} looks like the live install. Run on a copy, or pass --allow-real.",
            file=sys.stderr,
        )
        return 2

    try:
        if args.check:
            return do_check(client)
        if args.restore:
            return do_restore(client)
        return do_prepare(client, args.pack, rehash=not args.no_rehash)
    except (LookupError, ValueError) as e:
        print(f"client-prepare: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
