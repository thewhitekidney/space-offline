#!/usr/bin/env bash
# Bring a new client build dir into the repo's build registry, step by step.
# R-NEWBUILD §3 (NB-1, NB-2). Read-only on the install; never launches the
# client; the analysis container has no network.
#
#   tools/new-build.sh DIR [--prefix P] [--pinned NAME] [--go-on] [--data-done] [--report] [--dry-run]
#   tools/new-build.sh --gate-check        # NB-1: re-verify committed deltas
#
#   DIR        install dir: a name under the prefix (tq-new) or a path
#   --prefix   the EVE/ dir holding the installs (default: the offline
#              prefix copy, never the online one; or $EVE_PREFIX)
#   --pinned   the pinned install's dir name beside DIR (default tq-offline)
#   --go-on    continue past a `differs` verdict (it is still reported)
#   --data-done  step 7 (static data) has been done by hand; do not stop there
#   --report   after the steps, run tools/port-surface.sh, bundle-diff.py and
#              port-recheck.py (those present) and print their exit codes
#   --dry-run  print what each step would run, run nothing
#
# Steps (docs/guides/build-compat.md): 1 the dir is an install; 2 hash manifest
# (tools/pin-client.sh, into out/); 3 delta against the PINNED build;
# 4 `differs` reading; 5 loader layout (fsd.CFSD_BUILD_LAYOUTS) and expected
# counts; 6 registry row; 7 data; 8 patch manifest and the hosts check;
# 9 gate. Layout and registry come before the data: content.py refuses a
# build that has neither.
# A step that needs a human decision prints exactly what to do and the
# script exits 10. Exit: 0 all done, 1 a step failed, 2 usage, 10 stopped.
#
# `--gate-check` is what tools/gate.sh runs: for every committed
# findings/build-delta-<old>-<new>.tsv whose two installs both exist on
# this machine it runs build-delta.py --check; it skips with a note when
# they do not. Every delta is checked, not only those against the pinned
# build: after a re-pin the older deltas stay as history and must still
# reproduce. Exit 0 ok/skipped, 1 a delta no longer reproduces.
# Test hooks: EVE_ANALYSIS (default tools/analysis.sh), EVE_PREFIX.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

analysis=${EVE_ANALYSIS:-tools/analysis.sh}
registry=tools/builds.tsv

usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; }
die() { echo "new-build: $*" >&2; exit 2; }

prefixes() {
    if [[ -n ${EVE_PREFIX:-} ]]; then echo "$EVE_PREFIX"; return; fi
    echo "$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE"
    # Never the online prefix (compatdata/8500): a typo'd DIR must not fall
    # back into it. Pass --prefix or EVE_PREFIX for any other place.
}
ini_get() { sed -n "s/^$2 *= *//p" "$1/start.ini" 2>/dev/null | head -1 | tr -d '\r '; }
# find_install BUILD: print the first install dir whose start.ini says BUILD.
find_install() {
    local p d
    while read -r p; do
        for d in "$p"/*/; do
            d=${d%/}
            [[ -f $d/start.ini && $(ini_get "$d" build) == "$1" ]] && { echo "$d"; return 0; }
        done
    done < <(prefixes)
    return 1
}
pinned_build() { awk -F'\t' '!/^#/ && $3=="pinned" {print $1; exit}' "$registry"; }

if [[ ${1:-} == --gate-check ]]; then
    [[ -f $registry ]] || { echo "   build deltas: no $registry; skipped"; exit 0; }
    shopt -s nullglob
    files=(findings/build-delta-*-*.tsv)
    ((${#files[@]})) || { echo "   build deltas: none committed; skipped"; exit 0; }
    for f in "${files[@]}"; do
        pair=${f##*/build-delta-}; pair=${pair%.tsv}
        old=${pair%%-*}; new=${pair#*-}
        # a delta is old->new; the old side is the baseline it was taken against
        od=$(find_install "$old") || { echo "   $f: build $old not installed here; skipped"; continue; }
        nd=$(find_install "$new") || { echo "   $f: build $new not installed here; skipped"; continue; }
        if [[ $(dirname "$od") != "$(dirname "$nd")" ]]; then
            echo "   $f: installs are in different prefixes; skipped"; continue
        fi
        rc=0
        EVE_CLIENT_DIR=$(basename "$nd") EVE_CLIENT=$nd "$analysis" python3 -I tools/build-delta.py \
            "/client-root/$(basename "$od")" "/client-root/$(basename "$nd")" --check "$f" || rc=$?
        # 0 equivalent and 3 differs both mean the committed rows reproduce.
        if ((rc != 0 && rc != 3)); then
            echo "gate: build delta $f no longer reproduces (exit $rc); the install changed" >&2
            exit 1
        fi
        echo "   $f: reproduces (exit $rc)"
    done
    exit 0
fi

dir=""; prefix=""; pinned_name=tq-offline; goon=0; dry=0; datadone=0; report=0
while (($#)); do
    case "$1" in
        --prefix) [[ $# -ge 2 ]] || die "--prefix needs a value"; prefix=$2; shift ;;
        --pinned) [[ $# -ge 2 ]] || die "--pinned needs a value"; pinned_name=$2; shift ;;
        --go-on) goon=1 ;;
        --data-done) datadone=1 ;;
        --dry-run) dry=1 ;;
        --report) report=1 ;;
        -h|--help) usage; exit 0 ;;
        -*) die "unknown argument $1" ;;
        *) [[ -z $dir ]] || die "one DIR only"; dir=$1 ;;
    esac
    shift
done
[[ -n $dir ]] || { usage >&2; exit 2; }

# Resolve DIR: a path as given, else a name under the first prefix holding it.
if [[ $dir != */* && ! -d $dir ]]; then
    found=""
    if [[ -n $prefix ]]; then [[ -d $prefix/$dir ]] && found=$prefix/$dir
    else while read -r p; do [[ -d $p/$dir ]] && { found=$p/$dir; break; }; done < <(prefixes); fi
    [[ -n $found ]] && dir=$found
fi
dir=${dir%/}
run() { if ((dry)); then echo "   (dry-run) $*"; else "$@"; fi; }
stop() { echo; echo "new-build: STOP at step $1: needs you"; shift; printf '%s\n' "$@"
         echo "new-build: then run this command again."; exit 10; }

echo "== step 1: is $dir an install"
[[ -f $dir/start.ini ]] || die "$dir has no start.ini; not a client install"
build=$(ini_get "$dir" build); line=$(ini_get "$dir" codename)
[[ $build =~ ^[0-9]+$ ]] || die "no numeric build in $dir/start.ini"
parent=$(cd "$dir/.." && pwd); name=$(basename "$dir")
pin=$(pinned_build); [[ -n $pin ]] || die "no pinned build in $registry"
echo "   build $build, codename ${line:-?}, pinned build is $pin"
[[ $build != "$pin" ]] || die "$name is the pinned build itself"
[[ -f $parent/$pinned_name/start.ini ]] || die "pinned install $parent/$pinned_name not found (use --pinned)"
[[ $(ini_get "$parent/$pinned_name" build) == "$pin" ]] || die "$pinned_name is not build $pin"

echo "== step 2: hash manifest (read-only) into out/new-build/$build/"
run mkdir -p "out/new-build/$build"
run tools/pin-client.sh --write --path "$dir" --manifest "out/new-build/$build/pin.sha256"

echo "== step 3: delta against the pinned build $pin"
delta=findings/build-delta-$pin-$build.tsv
rc=0
if ((dry)); then
    echo "   (dry-run) EVE_CLIENT_DIR=$name EVE_CLIENT=$dir $analysis python3 -I tools/build-delta.py /client-root/$pinned_name /client-root/$name --out /work/$delta"
else
    EVE_CLIENT_DIR=$name EVE_CLIENT=$dir "$analysis" python3 -I tools/build-delta.py \
        "/client-root/$pinned_name" "/client-root/$name" --out "/work/$delta" || rc=$?
fi
case $rc in
    0) echo "   equivalent: $delta written; commit it" ;;
    3) echo "   DIFFERS: read the reason rows in $delta"
       ((goon)) || stop 4 "Read the 'verdict' row and the reason rows in $delta." \
         "Protocol file: re-diff blue.dll/_destiny.dll with tools/dis.py, recheck marshal opcodes." \
         "Call surface: compare findings/client-calls.tsv, client-callable.tsv, push-events.tsv, session-keys.tsv, then implement the calls (NB-5)." \
         "Schema: tools/pyd_schema.py vs docs/content/schema.md. Static data: step 7 below (NB-7)." \
         "To go on to the registry anyway (status stays unverified): add --go-on." ;;
    *) echo "new-build: delta failed (exit $rc)" >&2; exit 1 ;;
esac

echo "== step 5: loader layout and expected counts"
# `content.py`'s schema_guard raises for a build with no CFSD_BUILD_LAYOUTS
# entry, and content-lib.sh refuses an unregistered build, so both come
# BEFORE the data (step 7), not after it.
fsd=tools/fsd.py
if ! awk -v b="$build" '/^CFSD_BUILD_LAYOUTS = \{/ {on=1; next} on && /^\}/ {exit} on && $1 == b ":" {f=1} END {exit !f}' "$fsd" 2>/dev/null; then
    stop 5 "$build has no entry in fsd.CFSD_BUILD_LAYOUTS ($fsd): content.py refuses it." \
      "Regenerate only the loaders the delta says changed, as an overlay of the new base:" \
      "  tools/analysis.sh python3 tools/pyd_schema.py tsv --client /client --tables TABLE,TABLE --out findings/pyd-schemas-$build.tsv" \
      "(TABLE: the stems named in the schema rows of $delta; never overwrite the hand-corrected dungeons messageTextID row.)" \
      "Recheck every presence-bit fix by hand, then add one line to CFSD_BUILD_LAYOUTS (see its comment)." \
      "Counts: content_types.EXPECT_BY_BUILD and content_dungeons.EXPECT_BY_BUILD hold per-build row counts;" \
      "until a row is added, content.py --check NOTES an UNVERIFIED count instead of failing. After step 7" \
      "read those notes, then add the rows."
fi
echo "   build $build has a loader layout"

echo "== step 6: registry"
if ! awk -F'\t' -v b="$build" '!/^#/ && $1==b {f=1} END {exit !f}' "$registry"; then
    stop 6 "Add this row to $registry (status unverified until the human check):" \
      "  $build	${line:-V??.??}	unverified	-	build-delta $(date +%F): see $delta" \
      "and one entry in eve_marshal::build::BUILDS (the test the_table_matches_tools_builds_tsv checks both)." \
      "A new release line (new codename) also needs a new ReleaseLine and handshake identity." \
      "Set status equivalent only when step 3 said equivalent AND the human check passed."
fi
echo "   build $build is registered"

echo "== step 7: data"
echo "   only if step 3 said static data differs:"
echo "   tools/analysis.sh python3 tools/content.py ... --build $build; then --check --against OLDBUNDLE"
echo "   (a count change prints UNVERIFIED notes until EXPECT_BY_BUILD has the build; a data_hash difference: tools/bundle-diff.py OLD NEW if present)"
# A `static data` reason row (resfileindex or bin64/staticdata) means the
# content bundle must be re-extracted before the build is trusted.
if ((!dry)) && [[ -f $delta ]] && grep '^verdict.*differs' "$delta" | grep -q 'static data' && ((!datadone)); then
    stop 7 "$delta has 'static data' reasons: the content data changed." \
      "Re-extract and compare the bundle (docs/guides/build-compat.md, docs/content/plan.md)," \
      "tools/analysis.sh python3 tools/content.py ... --build $build, then --check --against OLDBUNDLE." \
      "When that is done, run this command again with --data-done."
fi

echo "== step 8: patch manifest and hosts"
echo "   CONTROL COPY first: the census needs a STOCK copy of $name, made before any patch:"
echo "   apply --tier B --only B0,P0 on a copy named $name-control (tools/census.sh reads the control name); once $name is patched there is no stock copy short of revert."
if [[ ! -f tools/patches/$build.json ]]; then
    stop 8 "Locate the recipes in the new build and merge them into the pinned manifest:" \
      "  tools/analysis.sh python3 -I tools/eve_patch_author.py locate /client --merge tools/patches/$pin.json > tools/patches/$build.json" \
      "Then change build, identify.start_ini_build and the comment; read the diff against $pin.json" \
      "(a refusal names the rule that did not fit its slot; B3 and the code.ccp members are the likely ones)." \
      "Verify: tools/analysis.sh python3 -I tools/eve_patch_author.py check /client --manifest tools/patches/$build.json" \
      "        and python3 -I tools/test_eve_patch.py"
fi
echo "   tools/patches/$build.json exists"
hrc=0
if ((dry)); then
    echo "   (dry-run) EVE_CLIENT_DIR=$name EVE_CLIENT=$dir $analysis python3 -I tools/eve_patch_author.py hosts /client"
else
    EVE_CLIENT_DIR=$name EVE_CLIENT=$dir "$analysis" python3 -I tools/eve_patch_author.py hosts /client || hrc=$?
    ((hrc == 0)) || stop 8 "The hosts check exited $hrc: a host not in out/census/known-hosts.txt (or no list)." \
      "Read each NEW line. An unreviewed outbound host means the jail and the patcher need a look before any client runs." \
      "Once reviewed, add it to the list (eve_patch_author.py hosts /client --write-known rewrites the whole list)."
fi

if ((report)); then
    echo "== report: one page"
    echo "   delta: $(grep '^verdict' "$delta" 2>/dev/null | cut -f3,6 | head -1 | tr '\t' ' ')"
    # T2-T4 (docs: r-port-1109 section 4). A tool that is not in the tree yet is named, not faked.
    oldpyc=${EVE_REPORT_OLD_PYC:-out/pyc}; newpyc=${EVE_REPORT_NEW_PYC:-out/new-build/$build/pyc}
    oldb=${EVE_REPORT_OLD_BUNDLE:-out/content}; newb=${EVE_REPORT_NEW_BUNDLE:-out/content-$build}
    rep() {  # NAME TOOL ARGS...
        local label=$1 tool=$2 rc=0; shift 2
        if [[ ! -x $tool ]]; then echo "   $label: $tool not present"; return; fi
        # A Python tool runs in the analysis container, like every other one here.
        if [[ $tool == *.py ]]; then "$analysis" python3 -I "$tool" "$@" >/dev/null 2>&1 || rc=$?
        else "$tool" "$@" >/dev/null 2>&1 || rc=$?; fi
        echo "   $label: exit $rc ($tool $*)"
    }
    if ((dry)); then echo "   (dry-run) surface, bundle and recheck are not run"
    else
        if [[ -d $oldpyc && -d $newpyc ]]; then rep surface tools/port-surface.sh "$oldpyc" "$newpyc"
        else echo "   surface: $oldpyc or $newpyc missing"; fi
        if [[ -d $oldb && -d $newb ]]; then rep bundle tools/bundle-diff.py "$oldb" "$newb"
        else echo "   bundle: $oldb or $newb missing"; fi
        recheck=("$oldpyc" "$newpyc")
        [[ -d $oldb && -d $newb ]] && recheck+=(--old-bundle "$oldb" --new-bundle "$newb")
        if [[ -x tools/port-recheck.py && ! ( -d $oldpyc && -d $newpyc ) ]]; then echo "   recheck: $oldpyc or $newpyc missing"
        else rep recheck tools/port-recheck.py "${recheck[@]}"; fi
    fi
fi

echo "== step 9: gate"
echo "   EVE_MAX_JOBS=6 tools/dev.sh cargo test --locked --workspace"
echo "   EVE_MAX_JOBS=6 tools/dev.sh cargo clippy --locked --workspace --all-targets -- -D warnings"
echo "   then tools/land.sh --head"
echo "new-build: all steps done for build $build"
