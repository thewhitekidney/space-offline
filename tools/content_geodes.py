"""The `geodes` content section: which of the client's crates are geodes.

Lane L6A GEODES (issue #32; `handback/r-nes-econ/report.md` §2.6 A). A real
`content.py` section, found by glob, so `tools/up.sh` extracts it into
whichever bundle it selects.

A geode is a crate (`crates.fsdbinary`, 637 rows, layout in
`findings/pyd-schemas.tsv`) that a mining cut may leave in the hold and a
pilot opens docked. The client already knows how to open a crate whose
`lootPresentationType` is `hacking` (the reveal window, `crateService
.OpenCrate`), and for one with no `fittings`, `skillBundleID` or
`fixedLootItems` the server's answer is the whole contents. Those are the
**candidates**; which of them are geodes is the owner's pick, by name, in
`content/econ/geode-picks.tsv` (`name`, `tier`, `why`). A pick names the
type's **en-us name exactly**; every type of that name becomes a geode of the
tier. **No real name or id is in the repository**: the picks file ships empty.
The owner's real picks go in the gitignored `out/geode-picks.local.tsv` (same
format), which is read as well when it exists.

Written into the bundle:

    mining/geodes.tsv             the picked types: `#type_id  tier`. Empty
                                  (header only) when nothing is picked, and
                                  then the server makes no geode.
    mining/geode-candidates.tsv   every candidate, for the owner to read and
                                  pick from: `type_id  name  group_id  volume`.

The derivation is pure (plain dicts in, rows out); the unit tests feed it
invented rows. `gather` is the only client reader.
"""
import os
import sys

SECTION = "geodes"
DEPENDS = ()
CFSD = ("crates", "types")

_HERE = os.path.dirname(os.path.abspath(__file__))

GEODES_HEADER = ("type_id", "tier")
CANDIDATES_HEADER = ("type_id", "name", "group_id", "volume")
PICKS_FILE = os.path.join(os.path.dirname(_HERE), "content", "econ", "geode-picks.tsv")
#: The same format, gitignored: the owner's picks, with their real names.
LOCAL_PICKS_FILE = os.path.join(os.path.dirname(_HERE), "out", "geode-picks.local.tsv")
SOURCES = ["res:/staticdata/crates.fsdbinary", "res:/staticdata/types.fsdbinary",
           "res:/localizationfsd/localization_fsd_en-us.pickle"]

#: The server's tiers (`eve-mod-econ` `tunables::PRIZES_BY_TIER`).
TIERS = (1, 2, 3)


def _clean(text):
    """One line, no tabs: a TSV field."""
    return " ".join(str(text or "").replace("\t", " ").split())


def parse_picks(lines):
    """`[(name, tier)]` from the picks file's lines.

    Comments (`#`) and blank lines are skipped; the header is the first line
    left. A tier outside `TIERS` is an error naming the line."""
    rows, header = [], None
    for n, line in enumerate(lines, 1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        fields = line.split("\t")
        if header is None:
            header = fields
            if header[:2] != ["name", "tier"]:
                raise ValueError("geode-picks.tsv: header must start `name<TAB>tier`, got %r"
                                 % line)
            continue
        if len(fields) < 2 or not fields[0].strip():
            raise ValueError("geode-picks.tsv line %d: %r" % (n, line))
        try:
            tier = int(fields[1])
        except ValueError:
            raise ValueError("geode-picks.tsv line %d: tier %r" % (n, fields[1]))
        if tier not in TIERS:
            raise ValueError("geode-picks.tsv line %d: tier %d is not one of %s"
                             % (n, tier, TIERS))
        rows.append((fields[0].strip(), tier))
    return rows


def is_candidate(crate):
    """A crate whose whole contents the server can answer with: random loot
    (`hacking` reveal), no fitting, skill bundle or fixed list."""
    return (crate.get("lootPresentationType") == "hacking"
            and not crate.get("fittings")
            and not crate.get("skillBundleID")
            and not crate.get("fixedLootItems"))


def derive(crates, types, text, picks):
    """`(geode rows, candidate rows, problems)`.

    `crates` is `{key: row}` (`typeID`, `nameID`, `lootPresentationType`,
    `fittings`, `skillBundleID`, `fixedLootItems`), `types` `{typeID: row}`
    (`groupID`, `volume`), `text(message_id)` the en-us string, `picks`
    `[(name, tier)]`.

    A candidate whose type the client has no `types` row for is skipped (it
    cannot be an item). A pick that matches no candidate, or one name picked
    at two tiers, is a problem: the extraction refuses rather than silently
    making no geode."""
    cands = {}
    for crate in crates.values():
        tid = crate.get("typeID")
        if tid not in types or not is_candidate(crate):
            continue
        cands[tid] = _clean(text(crate.get("nameID")))
    cand_rows = [(tid, cands[tid], types[tid].get("groupID"),
                  repr(float(types[tid].get("volume") or 0.0)))
                 for tid in sorted(cands)]
    problems, tier_of = [], {}
    for name, tier in picks:
        hit = [tid for tid, n in cands.items() if n == name]
        if not hit:
            problems.append("pick %r matches no candidate crate" % name)
            continue
        for tid in hit:
            if tier_of.get(tid, tier) != tier:
                problems.append("pick %r is at two tiers" % name)
            tier_of[tid] = tier
    rows = [(tid, tier_of[tid]) for tid in sorted(tier_of)]
    return rows, cand_rows, problems


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def gather(ctx):
    """The client's tables: `(crates, types)`."""
    fsd = _content()._imp("fsd")  # noqa: SLF001
    c = ctx.client
    return fsd.rows(c, "crates"), fsd.rows(c, "types")


def _picks(ctx):
    with open(ctx.input(PICKS_FILE), encoding="utf-8") as fh:
        picks = parse_picks(fh.read().splitlines())
    # The owner's own picks, kept out of the repository (`out/` is gitignored):
    # same format as the shipped file, read when it exists and added to it.
    if os.path.exists(LOCAL_PICKS_FILE):
        with open(ctx.input(LOCAL_PICKS_FILE), encoding="utf-8") as fh:
            picks += parse_picks(fh.read().splitlines())
    return picks


def _derive(ctx):
    crates, types = gather(ctx)
    return derive(crates, types, ctx.text, _picks(ctx))


def section(ctx):
    rows, cands, problems = _derive(ctx)
    if problems:
        raise ValueError("geodes: " + "; ".join(problems))
    ctx.note("geodes: %d geode types picked of %d candidate crates" % (len(rows), len(cands)))
    return [("mining/geodes.tsv", GEODES_HEADER, rows, SOURCES),
            ("mining/geode-candidates.tsv", CANDIDATES_HEADER, cands, SOURCES)]


def check(ctx):
    content = _content()
    rows, cands, problems = _derive(ctx)
    bad = list(problems)
    for rel, want in (("mining/geodes.tsv", rows), ("mining/geode-candidates.tsv", cands)):
        got = list(content.read_rows(ctx.path(rel)))
        if got != [content.encode_row(r).split("\t") for r in want]:
            bad.append("%s: differs from a fresh derive (%d vs %d rows)"
                       % (rel, len(got), len(want)))
    return bad
