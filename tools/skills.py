#!/usr/bin/env python3
"""Extract every skill from the client into a TSV the server loads.

The server's skill model (``server/crates/eve-server/src/skill.rs``) needs
four numbers per skill to train it the way the client counts it -- rank and
the two attributes -- plus the group and volume the item store needs to hold
an uninjected skillbook, and the prerequisites the queue checks. All of it is
CCP's data, so -- like ``tools/pi_statics.py`` -- this is a *reader* and the
output goes to ``out/`` (gitignored), never into the repository. The server
loads it with ``--skills FILE``; without the flag it falls back to the 44
hand-copied rows of ``skill::SKILLS``.

Usage (inside the analysis container)::

    tools/analysis.sh python3 /work/tools/skills.py \\
        --client /client --resfiles /client-root/ResFiles \\
        --out /work/out/skills.tsv

Sources, every one read through ``tools/dogma.py`` (layouts derived from the
client's own loader ``.pyd`` files, ``docs/re/dogma.md``):

==========================  =============================================
column                      source
==========================  =============================================
skills                      ``types`` whose group is in category 16
                            (``categorySkill``), ``published`` only --
                            the client refuses to inject an unpublished
                            one itself (``skillsvc.InjectSkillIntoBrain``,
                            ``ItemNotASkill``)
rank                        ``typedogma`` attribute 275
                            (``skillTimeConstant``)
primary / secondary         ``typedogma`` attributes 180 / 181
required                    ``requiredskillsfortypes`` ``{skill: level}``
name                        ``types.typeNameID`` through the en-us table
==========================  =============================================

Format, one record per line, tab-separated; ``#`` starts a comment::

    skill  typeID groupID rank primary secondary volume required name

``required`` is ``typeID=level`` joined with ``;`` (empty for none). ``name``
is last because it is the only free-text column.

Stdlib only, read-only on the client.
"""

from __future__ import annotations

# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse  # noqa: E402
import os  # noqa: E402
import sys  # noqa: E402

# `tools/` itself is already on the path -- the rotation above put it last.
import dogma  # noqa: E402
import fsd  # noqa: E402

#: ``categorySkill`` (``inventorycommon/const.pyc``) **[V]**.
CATEGORY_SKILL = 16
#: ``attributeSkillTimeConstant``, ``attributePrimaryAttribute``,
#: ``attributeSecondaryAttribute`` (``dogma/const.pyc``) **[V]**.
ATTR_RANK, ATTR_PRIMARY, ATTR_SECONDARY = 275, 180, 181


def _num(v):
    """``repr`` of a non-integral float, plain digits for an integral one."""
    v = float(v)
    return str(int(v)) if v == int(v) else repr(v)


def extract(client, resfiles):
    d = dogma.Dogma(client, resfiles)
    c = d.client
    groups = fsd.cfsd_rows(
        fsd.CfsdTable(c.path("res:/staticdata/groups.fsdbinary"), "groups"), "groups")
    types = dict(d.table("types").rows())
    names = d.type_names()
    defaults = {k: v.get("defaultValue") or 0.0 for k, v in d.attributes.items()}

    skill_ids = sorted(
        t for t, r in types.items()
        if groups.get(r["groupID"], {}).get("categoryID") == CATEGORY_SKILL
        and r.get("published")
    )
    wanted = set(skill_ids)
    attrs = {}
    for key, row in d.table("typedogma").rows():
        if key in wanted:
            attrs[key] = {e["attributeID"]: e["value"] for e in row["dogmaAttributes"]}
    required = {}
    for key, row in d.table("requiredskillsfortypes").rows():
        if key in wanted:
            required[key] = row["skills"]

    out = [
        "# eve-offline skills, written by tools/skills.py from the player's own",
        "# client install. CCP's data: never commit this file.",
    ]
    for t in skill_ids:
        r = types[t]
        a = attrs.get(t, {})
        get = lambda i: a.get(i, defaults.get(i, 0.0))  # noqa: E731
        name = names.get(t)
        name = name if isinstance(name, str) else "type %d" % t
        out.append("\t".join([
            "skill", str(t), str(r["groupID"]), _num(get(ATTR_RANK)),
            _num(get(ATTR_PRIMARY)), _num(get(ATTR_SECONDARY)),
            _num(r.get("volume") or 0.0),
            ";".join("%d=%d" % kv for kv in sorted(required.get(t, {}).items())),
            name.replace("\t", " "),
        ]))
    return "\n".join(out) + "\n", len(skill_ids)


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--client", default=os.environ.get("EVE_CLIENT"))
    p.add_argument("--resfiles", default=None)
    p.add_argument("--out", required=True)
    a = p.parse_args(argv)
    text, n = extract(a.client, a.resfiles)
    with open(a.out, "w", encoding="utf-8") as f:
        f.write(text)
    print("skills: %d skills -> %s" % (n, a.out), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
