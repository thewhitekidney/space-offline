#!/bin/bash
# Copy the achievement definitions the EVE client downloaded from CCP's CDN, from the
# cache directory you name, into out/cdn/achievements/, where the `achievements` content
# section reads them. It copies; it fetches nothing and looks in no folder on its own. Run
# it on the host (WSL), not in tools/analysis.sh: the container sees the repo as /work and
# nothing of the Windows side.
#
# The client writes the file only after a character has ENTERED THE GAME on Tranquility
# (character selection is not enough), under its settings directory:
#   %LOCALAPPDATA%\CCP\EVE\<install>_tranquility\cache\Storage\achievements\data.bin
#
# usage: tools/copy-cdn-achievements.sh CACHE_DIR
#   CACHE_DIR  that ...\cache\Storage\achievements directory, as a WSL path
# The file is CCP's: it stays in out/ (gitignored) and goes nowhere near git.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
dest="$here/../out/cdn/achievements"

if [ $# -ne 1 ]; then
    echo "usage: tools/copy-cdn-achievements.sh CACHE_DIR" >&2
    echo "  CACHE_DIR: a Tranquility client's cache/Storage/achievements directory (enter the" >&2
    echo "  game once on Tranquility first; character selection is not enough)" >&2
    exit 2
fi
src=$1

[ -f "$src/data.bin" ] || { echo "copy-cdn-achievements: no data.bin in $src" >&2; exit 1; }
if [ "$(dd if="$src/data.bin" bs=1 skip=4 count=4 2>/dev/null)" != "ACHV" ]; then
    echo "copy-cdn-achievements: $src/data.bin is not an achievements file (no ACHV)" >&2
    exit 1
fi
mkdir -p "$dest"
cp "$src/data.bin" "$dest/data.bin"
if [ -f "$src/version.txt" ]; then cp "$src/version.txt" "$dest/version.txt"; fi
echo "copy-cdn-achievements: $(cat "$dest/version.txt" 2>/dev/null || echo '?') from $src -> out/cdn/achievements/"
