#!/usr/bin/env bash
# Bring a frozen client build from the online install into the offline prefix.
#
#   tools/import-build.sh tq-1109            # copy tq -> tq-1109, link blobs, check, pin
#   tools/import-build.sh tq-1109 --check    # dry run: every refusal and every count, no write
#   tools/import-build.sh tq-1109 --sweep    # monthly: re-check the md5 of its blobs
#
# WHY. On the day before an expansion the online client (Steam appid 8500)
# holds the last build we can still test against. The next launcher start
# patches it away. This copies it out, once, by the rules of
# docs/guides/jail.md ("We run a copy") and tools/offline-prefix.sh (O19):
#
#   - the install (`tq`) becomes a REAL copy under the offline prefix, because
#     the client writes `cache:` inside its install and the patcher rewrites
#     files; a hard-linked install would write through to the online one;
#   - the ResFiles blobs the new indexes name are HARD-LINKED (a 48 G store
#     cannot be copied twice on this disk), never overwriting one, and each new
#     blob is checked against the index: size is field 4, md5 is field 3;
#   - the online prefix is only READ (open, stat, link(2) as the source);
#   - nothing is written outside the offline prefix and `out/new-build/<build>/`;
#   - a blob the online store lacks is listed as MISSING and the run exits
#     non-zero: the fix is "download all resources" in the launcher, never a
#     CDN fetch.
#
# Environment (tests point these at invented trees):
#   EVE_ONLINE_PREFIX   the online prefix (default $STEAM_ROOT/steamapps/compatdata/8500)
#   EVE_OFFLINE_PREFIX  the offline prefix (default tools/prefix-lib.sh's)
#   EVE_IMPORT_SRC_DIR  the install dir in the online prefix (default tq)
#   EVE_IMPORT_OUT      where provenance and the pin go (default out/new-build)
#
# Afterwards: tools/new-build.sh DIR --prefix <offline EVE dir>, and
# docs/guides/build-compat.md.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"

die() { echo "import-build: $*" >&2; exit 1; }
say() { printf '%s\n' "$*"; }

NAME="" CHECK=0 SWEEP=0
while (($#)); do
    case $1 in
        --check) CHECK=1 ;;
        --sweep) SWEEP=1 ;;
        -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; exit 0 ;;
        -*) echo "import-build: unknown argument $1" >&2; exit 2 ;;
        *) [[ -z $NAME ]] || { echo "import-build: one DIRNAME only" >&2; exit 2; }; NAME=$1 ;;
    esac
    shift
done
[[ -n $NAME ]] || { echo "usage: tools/import-build.sh DIRNAME [--check] [--sweep]" >&2; exit 2; }
((CHECK + SWEEP < 2)) || { echo "import-build: --check and --sweep are separate runs" >&2; exit 2; }

[[ $(id -u) != 0 ]] || die "run as yourself, not root"

# --- the name ----------------------------------------------------------------
[[ $NAME =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || die "bad directory name '$NAME' (letters, digits, . _ -)"
case $NAME in
    tq | sisi) die "'$NAME' is an online install name; a jailed process must never get one (docs/guides/jail.md)" ;;
    *.partial | *eveoffline-*) die "'$NAME' is reserved" ;;
esac

# --- the places --------------------------------------------------------------
: "${STEAM_ROOT:=$HOME/.local/share/Steam}"
: "${EVE_ONLINE_PREFIX:=$STEAM_ROOT/steamapps/compatdata/$EVE_ONLINE_PREFIX_ID}"
: "${EVE_OFFLINE_PREFIX:=$(eve_offline_prefix "$HOME")}"
: "${EVE_IMPORT_SRC_DIR:=tq}"
: "${EVE_IMPORT_OUT:=$REPO/out/new-build}"
EVE_REL=pfx/drive_c/CCP/EVE
ON=$(realpath -m -- "$EVE_ONLINE_PREFIX")
OFF=$(realpath -m -- "$EVE_OFFLINE_PREFIX")
ON_EVE=$ON/$EVE_REL
OFF_EVE=$OFF/$EVE_REL
SRC=$ON_EVE/$EVE_IMPORT_SRC_DIR
DST=$OFF_EVE/$NAME
STAGE=$DST.partial

# The destination is the offline prefix: made by offline-prefix.sh, nowhere
# near the online one, and every directory on the way a real one.
! eve_is_online_path "$OFF" || die "$OFF is inside an online prefix (compatdata/$EVE_ONLINE_PREFIX_ID)"
! eve_is_online_path "$DST" || die "$DST is inside an online prefix (compatdata/$EVE_ONLINE_PREFIX_ID)"
[[ -f $OFF/eve-offline-prefix.txt ]] || die "no offline prefix at $OFF (tools/offline-prefix.sh makes it once)"
[[ $(realpath -m -- "$OFF_EVE") == "$OFF_EVE" && -d $OFF_EVE && ! -L $OFF_EVE ]] \
    || die "$OFF_EVE is not a plain directory inside the offline prefix"
[[ $(realpath -m -- "$OFF_EVE/ResFiles") == "$OFF_EVE/ResFiles" && -d $OFF_EVE/ResFiles ]] \
    || die "$OFF_EVE/ResFiles is not a plain directory"

index_files() { # DIR: the index files that exist there
    local d=$1 f
    for f in resfileindex.txt resfileindex_Windows.txt resfileindex_prefetch.txt; do
        [[ -f $d/$f ]] && printf '%s\n' "$d/$f"
    done
}

# --- the blob pass, one python program for the three modes -----------------------
# mode link   : hard-link each blob the indexes name that the store lacks
# mode check  : the same, writing nothing
# mode sweep  : every indexed blob must be in the store and match its index row
# The index is DATA from outside: only a name matching the blob pattern is
# used, joined under the two ResFiles roots and nowhere else.
blob_pass() { # MODE ONLINE_RESFILES OFFLINE_RESFILES INDEX...
    python3 -I - "$@" <<'PY'
import hashlib, os, re, stat, sys

mode, on_root, off_root = sys.argv[1:4]
indexes = sys.argv[4:]
BLOB = re.compile(r"^[0-9a-f]{2}/[0-9a-f]{16}_[0-9a-f]{32}$")

rows = {}
bad_names = []
for ix in indexes:
    with open(ix, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            p = line.rstrip("\r\n").split(",")
            if len(p) < 5:
                continue
            rel = p[1]
            if not BLOB.match(rel):
                bad_names.append(rel)
                continue
            try:
                size = int(p[3])
            except ValueError:
                bad_names.append(rel)
                continue
            rows[rel] = (p[2], size)

def digest(path):
    h = hashlib.md5()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()

def regular(path):
    try:
        st = os.lstat(path)
    except OSError:
        return None
    return st if stat.S_ISREG(st.st_mode) else None

def verify(path, md5, size):
    st = regular(path)
    if st is None:
        return "not a regular file"
    if st.st_size != size:
        return f"size {st.st_size} != index {size}"
    if digest(path) != md5:
        return "md5 differs from the index"
    return None

have = linked = 0
missing, mismatch, absent = [], [], []
for rel in sorted(rows):
    md5, size = rows[rel]
    dst = os.path.join(off_root, rel)
    if mode == "sweep":
        if regular(dst) is None:
            absent.append(rel)
            continue
        why = verify(dst, md5, size)
        if why:
            mismatch.append((rel, why))
        else:
            have += 1
        continue
    if os.path.lexists(dst):
        have += 1                      # never overwritten, never re-checked here
        continue
    src = os.path.join(on_root, rel)
    if regular(src) is None:
        missing.append(rel)
        continue
    why = verify(src, md5, size)
    if why:
        mismatch.append((rel, why))
        continue
    if mode == "check":
        linked += 1
        continue
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    try:
        os.link(src, dst, follow_symlinks=False)
    except FileExistsError:
        have += 1
        continue
    linked += 1

for rel in bad_names:
    print("BAD-NAME", rel[:80])
for rel in missing:
    print("MISSING", rel)
for rel in absent:
    print("ABSENT", rel)
for rel, why in mismatch:
    print("MISMATCH", rel, why)
word = {"link": "linked", "check": "would link", "sweep": "linked"}[mode]
print(f"blobs: {len(rows)} indexed, {have} already in the store, "
      f"{linked} {word}, {len(missing)} MISSING, {len(absent)} absent, "
      f"{len(mismatch)} mismatched, {len(bad_names)} bad names")
sys.exit(1 if (missing or absent or mismatch or bad_names) else 0)
PY
}

ini_get() { sed -n "s/^$2 *= *//p" "$1/start.ini" | tr -d '\r' | head -1; }

# --- sweep: only the offline prefix is read -------------------------------------
if ((SWEEP)); then
    [[ -d $DST && ! -L $DST ]] || die "no install $DST to sweep"
    mapfile -t IX < <(index_files "$DST")
    ((${#IX[@]})) || die "$DST has no resfileindex.txt"
    say "== sweep: md5 and size of every blob $NAME names, in $OFF_EVE/ResFiles"
    rc=0
    blob_pass sweep "" "$OFF_EVE/ResFiles" "${IX[@]}" || rc=$?
    ((rc == 0)) && say "sweep: clean." || say "sweep: FAILED. A blob changed under us (a launcher run rewrote a shared inode?); relink it from the online store or re-import."
    exit "$rc"
fi

# --- refusals, before anything is written ---------------------------------------
eve_is_online_path "$SRC" || die "the source $SRC is not inside the online prefix (compatdata/$EVE_ONLINE_PREFIX_ID); set EVE_ONLINE_PREFIX"
[[ -f $SRC/start.ini && -f $SRC/resfileindex.txt ]] || die "no install at $SRC (start.ini and resfileindex.txt expected)"
[[ -d $ON_EVE/ResFiles && ! -L $ON_EVE/ResFiles ]] || die "no ResFiles store at $ON_EVE/ResFiles"
for d in "$DST" "$STAGE"; do
    if [[ -e $d || -L $d ]]; then
        die "$d already exists; not replacing it. (A .partial is a run that was cut off: remove it yourself.)"
    fi
done
[[ $(stat -c %d -- "$ON_EVE/ResFiles") == "$(stat -c %d -- "$OFF_EVE/ResFiles")" ]] \
    || die "ResFiles are on different filesystems; hard links cannot cross"

# Nothing may be running in the online prefix. One grep over every environ we
# can read; -z because the entries are NUL-separated (as offline-prefix.sh).
busy=""
while IFS= read -r e; do
    busy+=" ${e//[!0-9]/}"
done < <(grep -lzFx -e "STEAM_COMPAT_DATA_PATH=$ON" -e "STEAM_COMPAT_DATA_PATH=$ON/" \
    -e "WINEPREFIX=$ON/pfx" -e "WINEPREFIX=$ON/pfx/" /proc/[0-9]*/environ 2>/dev/null || true)
[[ -z $busy ]] || die "processes are using the online prefix (pids$busy). Close EVE and the launcher first."

links=$(find "$SRC" -type l -print -quit)
[[ -z $links ]] || die "the source holds a symlink ($links); a real copy must not follow one out of the install"
stock=$(find "$SRC" -name '*.eveoffline-*' -print -quit)
[[ -z $stock ]] || die "the source holds our own patch file ($stock); it is not stock"

build=$(ini_get "$SRC" build)
[[ $build =~ ^[0-9]+$ ]] || die "no numeric build in $SRC/start.ini"
codename=$(ini_get "$SRC" codename)
say "== source $SRC: build $build, codename ${codename:-?}"
say "== destination $DST"

need_k=$(du -sk -- "$SRC" | cut -f1)
free_k=$(df -k --output=avail -- "$OFF_EVE" | tail -1 | tr -d ' ')
((free_k > need_k)) || die "not enough space (copy needs ${need_k}K, ${free_k}K free)"

mapfile -t SRC_IX < <(index_files "$SRC")

if ((CHECK)); then
    say "== --check: nothing will be written"
    rc=0
    blob_pass check "$ON_EVE/ResFiles" "$OFF_EVE/ResFiles" "${SRC_IX[@]}" || rc=$?
    ((rc == 0)) && say "check: would import cleanly." || say "check: would FAIL (above)."
    exit "$rc"
fi

# --- the install: a real copy into a .partial, renamed only when whole ---------
cleanup() { [[ -n ${STAGE_MADE:-} && -d $STAGE ]] && rm -rf -- "$STAGE"; return 0; }
trap cleanup EXIT
STAGE_MADE=1
say "== copying the install (a real copy, never links)"
cp -a --reflink=auto -- "$SRC" "$STAGE"
[[ -z $(find "$STAGE" -type l -print -quit) ]] || die "a symlink appeared in the copy"
[[ $(find "$SRC" -type f | wc -l) == "$(find "$STAGE" -type f | wc -l)" ]] || die "the copy has a different file count"
[[ $(du -sb -- "$SRC" | cut -f1) == "$(du -sb -- "$STAGE" | cut -f1)" ]] || die "the copy has a different size"

# --- ResFiles: link the blobs, then rename --------------------------------------
say "== ResFiles: hard-linking the blobs the indexes name"
mapfile -t STAGE_IX < <(index_files "$STAGE")
BLOBLOG=$(mktemp)
rc=0
blob_pass link "$ON_EVE/ResFiles" "$OFF_EVE/ResFiles" "${STAGE_IX[@]}" >"$BLOBLOG" || rc=$?
cat "$BLOBLOG"
if ((rc != 0)); then
    rm -f -- "$BLOBLOG"
    say "import-build: FAILED; $NAME was not created (blobs linked so far stay, harmlessly)."
    say "  For MISSING: run the launcher, turn on 'download all resources', let it finish, close it, re-run."
    exit "$rc"
fi
mv -T -- "$STAGE" "$DST"
STAGE_MADE=""

# --- verify, pin, provenance ----------------------------------------------------
say "== check-resources"
cr=0
EVE_CLIENT=$DST EVE_RESFILES=$OFF_EVE/ResFiles "$REPO/tools/check-resources.sh" || cr=$?
mkdir -p -- "$EVE_IMPORT_OUT/$build"
say "== pin"
pr=0
"$REPO/tools/pin-client.sh" --write --path "$DST" --manifest "$EVE_IMPORT_OUT/$build/pin.sha256" || pr=$?
{
    echo "# import-build provenance (kept out of the install: build-delta would count a new root file as added)"
    echo "name:        $NAME"
    echo "build:       $build"
    echo "codename:    ${codename:-?}"
    echo "source:      $SRC"
    echo "destination: $DST"
    echo "date:        $(date -Is)"
    echo "user:        $(id -un)"
    echo "files:       $(find "$DST" -type f | wc -l)"
    echo "install:     real copy; ResFiles blobs hard-linked from the online store"
    grep '^blobs:' "$BLOBLOG"
    echo "check-resources exit: $cr"
    echo "pin-client exit:      $pr"
} >"$EVE_IMPORT_OUT/$build/import.txt"
rm -f -- "$BLOBLOG"
say "import-build: provenance in $EVE_IMPORT_OUT/$build/import.txt"
((cr == 0 && pr == 0)) || die "check-resources ($cr) or pin ($pr) failed"
say "import-build: done. Next: tools/new-build.sh $DST --prefix $OFF_EVE"
