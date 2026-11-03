"""The `deploy` content section: the deployables' space components and the
skill injectors' item rows.

Lane hand-data (issue #109). A real `content.py` section, found by glob, so
`tools/up.sh` extracts it into whichever bundle it selects. It moves two sets
of figures out of the Rust source, where they were hand-copied from the
client, into the bundle:

    deploy/units.tsv       the `spacecomponentsbytype` row of the **base** type
                           of the Mobile Depot and Mobile Tractor Unit groups
                           (`eve-server` `mtu::spec`), one row per group; and
                           (NW-4) the Mobile Cynosural Inhibitor, group 1249,
                           whose `cynoInhibitor.range` is column 11 and
                           its placement figures (own-group gap, gate and station
                           distance, max security) come from its `deploy`
                           component (`mtu::inhibitor`)
    deploy/injectors.tsv   the item rows of the Skill Extractor, the Large and
                           the Small Skill Injector (`types` and `groups`):
                           `skill_trading::TYPES`

The base type of a group is **[A]** the lowest type id of the group that has
every component the server reads (`cargoBay`, `activate`, `decay`, `deploy`);
falsifier: a group whose lowest such type differs from a later one in a figure
the server reads (the section lists the figures of every type of the group in
its note when they disagree). The derivation is pure (plain dicts in, rows
out); the unit tests feed it invented rows. `gather` is the only client reader.
"""
import os
import sys

SECTION = "deploy"
DEPENDS = ()
CFSD = ("spacecomponentsbytype", "types", "groups")

_HERE = os.path.dirname(os.path.abspath(__file__))

UNITS_HEADER = ("group_id", "activate_s", "decay_s", "access_range", "allow_user_add",
                "deploy_at", "own_group_gap", "tractor_max", "tractor_min", "cycle_s", "inhibit_range",
                "min_gate_dist", "max_security")
INJECTORS_HEADER = ("type_id", "group_id", "category_id", "singleton", "volume", "name")
SOURCES = ["res:/staticdata/spacecomponentsbytype.fsdbinary",
           "res:/staticdata/types.fsdbinary", "res:/staticdata/groups.fsdbinary",
           "res:/localizationfsd/localization_fsd_en-us.pickle"]

#: `groupMobileHomes` and `groupAutoLooter` [V] (`mtu::GROUP_MOBILE_DEPOT`,
#: `GROUP_MTU`).
UNIT_GROUPS = (1246, 1250)
#: `groupCynoInhibitor` [V] (`inventorycommon/const.py:1053`, `cyno::GROUP_INHIBITOR`).
INHIBITOR_GROUP = 1249
#: The components its base type must have; the range is `cynoInhibitor.range`
#: [V] (`spacecomponents/client/components/cynoInhibitor.py`). `decay` is
#: optional (the server then takes one hour).
INHIBITOR_REQUIRED = ("activate", "cynoInhibitor")
#: The components `mtu::spec` reads from every unit group's base type.
REQUIRED = ("cargoBay", "activate", "decay", "deploy")
#: `typeSkillExtractor`, `typeSkillInjector`, `typeSmallSkillInjector` [V]
#: (`skill_trading::TYPE_*`).
INJECTOR_TYPES = (40519, 40520, 45635)


def _clean(text):
    """One line, no tabs: a TSV field."""
    return " ".join(str(text or "").replace("\t", " ").split())


def _num(v):
    """An int stays an int, anything else a float; absent is 0."""
    if isinstance(v, bool):
        return int(v)
    if isinstance(v, int):
        return v
    return float(v or 0.0)


def unit_row(group, comp):
    """One `deploy/units.tsv` row from a type's component dicts."""
    cargo, deploy = comp["cargoBay"], comp["deploy"]
    tr, lo = comp.get("autoTractorBeam"), comp.get("autoLooter")
    # The tractor and the looter go together; the looter has no range of its
    # own, so a unit with a beam reads its cycle from the beam.
    beam = (_num(tr.get("maxRange")), _num(tr.get("minRange")),
            _num(tr.get("cycleTimeSeconds"))) if tr and lo else ("", "", "")
    return (group, _num(comp["activate"].get("durationSeconds")),
            _num(comp["decay"].get("durationSeconds")),
            _num(cargo.get("accessRange")), 1 if cargo.get("allowUserAdd") else 0,
            _num(deploy.get("deployAtRange")), _num(deploy.get("minDistanceFromOwnGroup")),
            *beam, "", "", "")


def inhibitor_row(group, comp):
    """The `deploy/units.tsv` row of the cyno inhibitor group: no cargo bay,
    no deploy distance; `own_group_gap` is `deploy.minDistanceFromOwnGroup`,
    the range `cynoInhibitor.range`, then `deploy.minDistanceFromStargatesAndStations`
    and `deploy.maxSecurityLevel` (empty when the type has none)."""
    deploy = comp.get("deploy") or {}

    def opt(key):
        v = deploy.get(key)
        return "" if v is None else _num(v)

    return (group, _num(comp["activate"].get("durationSeconds")),
            _num((comp.get("decay") or {}).get("durationSeconds")),
            0, 0, 0, _num(deploy.get("minDistanceFromOwnGroup")), "", "", "",
            _num(comp["cynoInhibitor"].get("range")),
            opt("minDistanceFromStargatesAndStations"), opt("maxSecurityLevel"))


def derive_units(components, types):
    """`(rows, problems)`. `components` is `{type_id: {component: dict}}`,
    `types` `{type_id: {"groupID": g}}`."""
    rows, problems = [], []
    for group in UNIT_GROUPS:
        ids = sorted(t for t, r in types.items() if r.get("groupID") == group
                     and t in components and all(components[t].get(c) for c in REQUIRED))
        if not ids:
            problems.append("group %d has no type with %s" % (group, ", ".join(REQUIRED)))
            continue
        rows.append(unit_row(group, components[ids[0]]))
        for t in ids[1:]:
            if unit_row(group, components[t])[1:] != rows[-1][1:]:
                print("deploy: group %d type %d differs from its base type %d "
                      "(the server serves the base row for the group)"
                      % (group, t, ids[0]), file=sys.stderr)
    ids = sorted(t for t, r in types.items() if r.get("groupID") == INHIBITOR_GROUP
                 and t in components
                 and all(components[t].get(c) for c in INHIBITOR_REQUIRED)
                 and _num(components[t]["cynoInhibitor"].get("range")) > 0)
    if not ids:
        # Not a problem: the server then refuses to launch an inhibitor, and
        # the rest of the section is still good (the component names are [V]
        # from the client's code but unread from its data until the bundle is
        # regenerated).
        print("deploy: group %d has no type with %s and a range; no inhibitor row"
              % (INHIBITOR_GROUP, ", ".join(INHIBITOR_REQUIRED)), file=sys.stderr)
    else:
        rows.append(inhibitor_row(INHIBITOR_GROUP, components[ids[0]]))
        for t in ids[1:]:
            if inhibitor_row(INHIBITOR_GROUP, components[t])[1:] != rows[-1][1:]:
                print("deploy: group %d type %d differs from its base type %d "
                      "(the server serves the base row for the group)"
                      % (INHIBITOR_GROUP, t, ids[0]), file=sys.stderr)
    return rows, problems


def derive_injectors(types, groups, text):
    """`(rows, problems)`: `groups` is `{group_id: category_id}`, `text` the
    en-us string of a message id."""
    rows, problems = [], []
    for tid in INJECTOR_TYPES:
        r = types.get(tid)
        if r is None:
            problems.append("type %d is not in the client's types" % tid)
            continue
        cat = groups.get(r.get("groupID"))
        if cat is None:
            problems.append("type %d: group %s is not in the client's groups"
                            % (tid, r.get("groupID")))
            continue
        rows.append((tid, r["groupID"], cat, 0, float(r.get("volume") or 0.0),
                     _clean(text(r.get("typeNameID")))))
    return rows, problems


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def gather(ctx):
    """The client's tables: `(components, types, groups)`."""
    fsd = _content()._imp("fsd")  # noqa: SLF001
    c = ctx.client
    types = fsd.rows(c, "types")
    components = {t: {k: v for k, v in dict(r).items() if v is not None}
                  for t, r in fsd.rows(c, "spacecomponentsbytype").items()
                  if types.get(t, {}).get("groupID") in UNIT_GROUPS + (INHIBITOR_GROUP,)}
    groups = {g: r["categoryID"] for g, r in fsd.rows(c, "groups").items()}
    return components, types, groups


def _derive(ctx):
    components, types, groups = gather(ctx)
    units, p1 = derive_units(components, types)
    inj, p2 = derive_injectors(types, groups, ctx.text)
    return units, inj, p1 + p2


def section(ctx):
    units, inj, problems = _derive(ctx)
    if problems:
        raise ValueError("deploy: " + "; ".join(problems))
    ctx.note("deploy: %d deployable groups, %d injector rows" % (len(units), len(inj)))
    return [("deploy/units.tsv", UNITS_HEADER, units, SOURCES),
            ("deploy/injectors.tsv", INJECTORS_HEADER, inj, SOURCES)]


def check(ctx):
    content = _content()
    units, inj, problems = _derive(ctx)
    bad = list(problems)
    for rel, want in (("deploy/units.tsv", units), ("deploy/injectors.tsv", inj)):
        got = list(content.read_rows(ctx.path(rel)))
        if got != [content.encode_row(r).split("\t") for r in want]:
            bad.append("%s: differs from a fresh derive (%d vs %d rows)"
                       % (rel, len(got), len(want)))
    return bad
