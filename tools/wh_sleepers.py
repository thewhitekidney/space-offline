#!/usr/bin/env python3
"""Write `out/content/wormholes/sleepers.tsv` by hand (Y-WH; client data, never in the repo).

The logic now lives in `content_wormholes.derive` (the `wormholes` section of
`content.py` writes this file in a full regeneration); this is the one-file
command line over it.

    python3 -I tools/wh_sleepers.py out/content out/content/wormholes/sleepers.tsv
"""
import os
import sys

sys.path.append(os.path.dirname(os.path.abspath(__file__)))
import content_npc  # noqa: E402
import content_wormholes as W  # noqa: E402


def main(bundle, out):
    def rows(rel):
        with open(os.path.join(bundle, rel), encoding="utf-8") as fh:
            return [l.rstrip("\n").split("\t") for l in fh if l.strip() and not l.startswith("#")]

    lines = W.derive(rows("types/typedata.tsv"), W.wrecks_of(rows("types/market.tsv")),
                     content_npc.needs_of, content_npc.role_of)
    with open(out, "w", encoding="utf-8") as fh:
        fh.write("\n".join(lines) + "\n")
    print(len(lines) - len(W.SLEEPER_COMMENTS) - 1, "rows")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
