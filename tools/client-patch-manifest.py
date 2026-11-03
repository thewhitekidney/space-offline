#!/usr/bin/env python3
"""Neutralise the manifest integrity self-check in a *copy* of blue.dll.

Why this is needed. The client refuses to start unless `manifest.dat` and the
files it lists match their recorded SHA-256 hashes. That self-check is what
makes a private server impossible without it: `start.ini` must say
`cryptoPack = Placebo` (docs/reference/login-path.md section 3) or the handshake cannot
complete, but any edit to `start.ini` — or to the manifest that records its
hash — fails the check. Re-hashing the manifest does not help: editing the
manifest breaks a second, whole-manifest integrity check on top of the per-file
hashes (established empirically: stock launches, any edit does not).

This tool makes the owner's own copy of `blue.dll` accept the owner's own
modified files. It is the client-modification the owner authorised for a
private, non-redistributed server. It does NOT touch the live install, is
reversible, and verifies its own work.

What it changes, exactly. The function that runs the check tries
`root:/manifest.dat` first and falls through to `bin:/manifest.dat` only when
the first returns a non-empty error; the client's "Verification Failure"
dialog is that fall-through. Right after the verify call, the error length is
loaded into rbx and later tested:

    call   VerifyManifestFile          ; verify root:/manifest.dat
    mov    rbx, [rax+0x10]             ; rbx = error length  <-- patch target
    ...
    test   rbx, rbx
    je     success                     ; taken only when rbx == 0

rbx is not touched between the load and the test, so replacing the 4-byte
`mov rbx,[rax+0x10]` (48 8B 58 10) with `xor rbx,rbx; nop` (48 31 DB 90) forces
the error length to zero and the success branch is always taken. One 4-byte
change, no jump targets moved. Nothing else in the binary is altered.

It is located by the stable `root:/manifest.dat` string, not a fixed offset, so
it finds the site on this build regardless of where it sits — and it REFUSES
rather than guesses if the surrounding instructions are not exactly what is
expected, so a different build fails safe instead of being corrupted.

Pairs with client-prepare.py --no-rehash: once the check is neutralised, set
`cryptoPack = Placebo` and leave the manifest stock.

Usage:
    tools/client-patch-manifest.py <client-dir> [--allow-real]
    tools/client-patch-manifest.py <client-dir> --check
    tools/client-patch-manifest.py <client-dir> --restore
"""

import argparse
import sys
from pathlib import Path

try:
    import pefile
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs
except ImportError:
    print("needs pefile + capstone; run via tools/client-patch-manifest.sh", file=sys.stderr)
    sys.exit(2)

DLL = "bin64/blue.dll"
BACKUP_SUFFIX = ".eveoffline-orig"

# The anchor string and the instruction we rewrite.
ANCHOR = b"root:/manifest.dat"
ORIG = bytes.fromhex("488b5810")  # mov rbx, [rax+0x10]
PATCHED = bytes.fromhex("4831db90")  # xor rbx, rbx ; nop


def rva_to_off(pe, rva):
    for s in pe.sections:
        size = max(s.Misc_VirtualSize, s.SizeOfRawData)
        if s.VirtualAddress <= rva < s.VirtualAddress + size:
            return s.PointerToRawData + (rva - s.VirtualAddress)
    return None


def find_string_va(pe, data, needle):
    """VA of `needle` in an initialised section, or None. Must be unique."""
    base = pe.OPTIONAL_HEADER.ImageBase
    hits = []
    start = 0
    while True:
        i = data.find(needle, start)
        if i < 0:
            break
        # Map file offset back to a VA.
        for s in pe.sections:
            lo = s.PointerToRawData
            hi = lo + s.SizeOfRawData
            if lo <= i < hi:
                hits.append(base + s.VirtualAddress + (i - lo))
                break
        start = i + 1
    return hits


def find_patch_site(pe, data):
    """Return the file offset of the `mov rbx,[rax+0x10]` to patch.

    Found structurally, not by offset: locate the code that references
    `root:/manifest.dat`, then the `mov rbx,[rax+0x10]` that follows a call and
    is itself followed by `test rbx,rbx; je`. Raises if the shape is not an
    exact match, so a build we do not recognise is refused rather than damaged.
    """
    base = pe.OPTIONAL_HEADER.ImageBase
    str_vas = find_string_va(pe, data, ANCHOR)
    if len(str_vas) != 1:
        raise LookupError(f"expected one {ANCHOR!r}, found {len(str_vas)}")
    target = str_vas[0]

    text = next(s for s in pe.sections if b".text" in s.Name)
    tstart = text.PointerToRawData
    tva = base + text.VirtualAddress
    code = data[tstart : tstart + text.SizeOfRawData]
    md = Cs(CS_ARCH_X86, CS_MODE_64)

    # 1. Find the `lea r?, [rip+disp32]` whose target is the anchor string, by
    #    scanning raw bytes. Linear disassembly of a whole .text section
    #    desyncs on embedded data and jump tables and can miss the site; a byte
    #    scan for the rip-relative lea encodings does not.
    import struct

    lea_prefixes = {
        (0x48, 0x8D, 0x15),  # lea rdx, [rip+d]
        (0x48, 0x8D, 0x05),  # lea rax
        (0x48, 0x8D, 0x0D),  # lea rcx
        (0x48, 0x8D, 0x1D),  # lea rbx
        (0x48, 0x8D, 0x35),  # lea rsi
        (0x48, 0x8D, 0x3D),  # lea rdi
        (0x4C, 0x8D, 0x05),  # lea r8
        (0x4C, 0x8D, 0x0D),  # lea r9
    }
    ref_vas = []
    for o in range(len(code) - 7):
        if (code[o], code[o + 1], code[o + 2]) in lea_prefixes:
            disp = struct.unpack_from("<i", code, o + 3)[0]
            if tva + o + 7 + disp == target:
                ref_vas.append(tva + o)
    if not ref_vas:
        raise LookupError("no code references root:/manifest.dat")
    if len(ref_vas) != 1:
        raise LookupError(f"root:/manifest.dat referenced from {len(ref_vas)} sites")
    ref_va = ref_vas[0]

    # 2. From there, walk forward to the first `call` whose next instruction is
    #    `mov rbx, [rax+0x10]`, and confirm a `test rbx,rbx; je` follows within
    #    the function body. Bounded so we never wander off.
    window_off = rva_to_off(pe, ref_va - base)
    window = data[window_off : window_off + 0x200]
    insns = list(md.disasm(window, ref_va))
    for i, ins in enumerate(insns[:-2]):
        if ins.mnemonic != "call":
            continue
        nxt = insns[i + 1]
        # The instruction after the verify call is `mov rbx,[rax+0x10]`
        # (unpatched) or, once patched, `xor rbx, rbx` followed by a nop. Match
        # either, so the site is found in both states and --check works on an
        # already-patched copy.
        is_orig = nxt.bytes == ORIG
        is_patched = nxt.mnemonic == "xor" and nxt.op_str == "rbx, rbx"
        if not (is_orig or is_patched):
            continue
        # Confirm the `test rbx,rbx` / `je` shape downstream. It sits ~22
        # instructions past the call (the std::string teardown of the error
        # buffer is between), so the window has to be generous.
        tail = insns[i + 2 : i + 45]
        test_at = next(
            (k for k, t in enumerate(tail) if t.mnemonic == "test" and t.op_str == "rbx, rbx"),
            None,
        )
        saw_je = test_at is not None and any(t.mnemonic == "je" for t in tail[test_at : test_at + 2])
        if test_at is not None and saw_je:
            off = rva_to_off(pe, nxt.address - base)
            if data[off : off + 4] not in (ORIG, PATCHED):
                raise LookupError("bytes at the located site are neither the expected mov nor the patch")
            return off
    raise LookupError("could not find the manifest-check branch; unrecognised build")


def _rip_target(ins):
    frag = ins.op_str.split("rip")[1]
    try:
        sign = 1
        frag = frag.strip()[1:]  # drop the + or -
        if ins.op_str.split("rip")[1].strip()[0] == "-":
            sign = -1
        hexpart = frag.split("]")[0].strip()
        return ins.address + ins.size + sign * int(hexpart, 16)
    except Exception:
        return None


def status(dll_path: Path):
    data = dll_path.read_bytes()
    pe = pefile.PE(str(dll_path), fast_load=True)
    off = find_patch_site(pe, data)
    cur = data[off : off + 4]
    if cur == ORIG:
        return off, "unpatched"
    if cur == PATCHED:
        return off, "patched"
    return off, f"unexpected ({cur.hex()})"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("client", type=Path)
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--restore", action="store_true")
    ap.add_argument("--allow-real", action="store_true")
    args = ap.parse_args()

    dll = args.client / DLL
    if not dll.is_file():
        print(f"no {DLL} in {args.client}", file=sys.stderr)
        return 2
    if args.client.name == "tq" and not args.allow_real:
        print(f"{args.client} looks like the live install; run on a copy or pass --allow-real.", file=sys.stderr)
        return 2

    backup = dll.with_name(dll.name + BACKUP_SUFFIX)

    try:
        off, state = status(dll)
    except LookupError as e:
        print(f"client-patch-manifest: {e}", file=sys.stderr)
        return 1

    if args.check:
        print(f"blue.dll:        {dll}")
        print(f"patch site:      file offset {off:#x}")
        print(f"state:           {state}")
        print(f"backup present:  {backup.exists()}")
        return 0

    if args.restore:
        if not backup.exists():
            print("no backup to restore from.", file=sys.stderr)
            return 1
        dll.write_bytes(backup.read_bytes())
        print(f"restored blue.dll from {backup.name}")
        return 0

    # Patch.
    if state == "patched":
        print("already patched; nothing to do.")
        return 0
    if state != "unpatched":
        print(f"refusing: site holds {state}, not the expected instruction.", file=sys.stderr)
        return 1

    if not backup.exists():
        backup.write_bytes(dll.read_bytes())
    data = bytearray(dll.read_bytes())
    data[off : off + 4] = PATCHED
    dll.write_bytes(data)

    _, new_state = status(dll)
    if new_state != "patched":
        dll.write_bytes(backup.read_bytes())
        print("verification failed; restored from backup.", file=sys.stderr)
        return 1

    print(f"patched {dll}")
    print(f"  at file offset {off:#x}: 48 8b 58 10  ->  48 31 db 90")
    print(f"  (mov rbx,[rax+0x10]  ->  xor rbx,rbx; nop)")
    print(f"  backup: {backup.name}  (undo with --restore)")
    print()
    print("The manifest self-check now always reports success on this copy, so")
    print("start.ini can be set to Placebo with the manifest left stock:")
    print("    tools/client-prepare.sh --no-rehash")
    return 0


if __name__ == "__main__":
    sys.exit(main())
