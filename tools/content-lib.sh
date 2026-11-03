# Which content bundle a client build may use, and what the server must be
# told about it. SOURCED, never run: tools/up.sh and tools/client-ci.sh read
# it, so the choice is made in one place.
#
# The bundle at out/content is the PINNED build's (and any build
# tools/builds.tsv calls `equivalent`: the same data). A build that is only
# `unverified` has loader layouts nobody has proved equal, so it must never
# extract into, or boot from, out/content: it gets a directory of its own,
# out/content-<build>. A mistake here costs the good bundle, which takes
# minutes to rebuild and cannot be told from a good one by looking.
#
# eve_content_select CLIENT_DIR REPO sets, on success:
#   EVE_CBUILD         the client's start.ini `build` (empty: no start.ini)
#   EVE_CSTATUS        its tools/builds.tsv status (`legacy` with no start.ini)
#   CONTENT_REL        the bundle directory, relative to REPO (under out/)
#   CONTENT            the same, absolute
#   EVE_SERVER_BUILD_ARGS   (--client-build B) for an unverified build, else ()
#   EVE_CONTENT_TOOL_ARGS   (--build B) for content.py, an unverified build
# and returns 1, saying why on stderr, when the pair is not safe.
#
# EVE_CONTENT picks the directory (REPO-relative or absolute, under REPO/out/);
# EVE_BUILDS_TSV replaces the registry (tests).

eve_content_select() {
    local client=$1 repo=$2 reg ini line status want
    EVE_CBUILD= EVE_CSTATUS=legacy EVE_SERVER_BUILD_ARGS=() EVE_CONTENT_TOOL_ARGS=()
    reg=${EVE_BUILDS_TSV:-$repo/tools/builds.tsv}
    ini=$client/start.ini
    if [[ -r $ini ]]; then
        EVE_CBUILD=$(sed -n 's/^[[:space:]]*build[[:space:]]*=[[:space:]]*//p' "$ini" | head -n 1 | tr -d '\r[:space:]')
        [[ $EVE_CBUILD =~ ^[0-9]+$ ]] || {
            echo "content-lib: $ini has no numeric 'build' line; refusing to pick a content bundle for it" >&2
            return 1
        }
        status=$(awk -F'\t' -v b="$EVE_CBUILD" '!/^#/ && $1 == b { print $3; exit }' "$reg" 2>/dev/null)
        [[ -n $status ]] || {
            echo "content-lib: client build $EVE_CBUILD is not in $reg; measure it with tools/build-delta.py and register it first" >&2
            return 1
        }
        EVE_CSTATUS=$status
    fi

    want=out/content
    case $EVE_CSTATUS in
        pinned | equivalent | legacy) ;;
        *) want=out/content-$EVE_CBUILD
           EVE_SERVER_BUILD_ARGS=(--client-build "$EVE_CBUILD")
           EVE_CONTENT_TOOL_ARGS=(--build "$EVE_CBUILD") ;;
    esac
    if [[ -n ${EVE_CONTENT:-} ]]; then
        want=${EVE_CONTENT#"$repo"/}
    fi
    case $want in
        /* | *..*) echo "content-lib: EVE_CONTENT=$want must be a directory under $repo/out/, written without '..'" >&2; return 1 ;;
        out/?*) ;;
        *) echo "content-lib: EVE_CONTENT=$want must be a directory under $repo/out/" >&2; return 1 ;;
    esac
    want=${want%/}
    if [[ $want == out/content && $EVE_CSTATUS != pinned && $EVE_CSTATUS != equivalent && $EVE_CSTATUS != legacy ]]; then
        echo "content-lib: client build $EVE_CBUILD is '$EVE_CSTATUS'; it must not use out/content (the pinned build's bundle). Use out/content-$EVE_CBUILD" >&2
        return 1
    fi
    CONTENT_REL=$want
    CONTENT=$repo/$want
    return 0
}
