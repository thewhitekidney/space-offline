"""The `police` content section: the faction police forces (NW-7, issue #157).

Writes `npc/police.tsv` (`docs/content/schema.md` §2): one row per force,
`faction_id, warning_key, type_ids`. The types are the client's group 182
(the police and CONCORD SWAT ships) in `types/typedata.tsv`, assigned to a
faction by the prefix of the type's name; the prefix table below is the only
thing the repository holds, and no row. `concord.rs` reads the table through
`npc_data` (the lead is found by its effects, the owner is the faction's own
NPC corporation, and the CONCORD row is the force of a system no faction
polices); with no bundle it falls back to invented fixtures.

The derivation is pure (plain rows in, rows out), so
`tools/test_content_police.py` feeds it invented names.
"""
import os
import sys

SECTION = "police"
DEPENDS = ("types",)

_HERE = os.path.dirname(os.path.abspath(__file__))

HEADER = ("faction_id", "warning_key", "type_ids")

#: The client's group of the police and SWAT ships [V-data].
GROUP = 182

#: [A] The warning of a force with no chat key of its own (Khanid, Ammatar
#: and CONCORD's SWAT have none in the client's strings). Falsifier: a client
#: capture of one of their arrivals, or a `chat*PoliceWarning` key for them.
DEFAULT_WARNING = "chatDefaultPoliceWarning"

#: Type-name prefix -> (faction id, warning key) [V-data names]. 500006 is the
#: CONCORD Assembly, whose row is the default force; Sarum, Roden and the
#: other group-182 families have no system to police and are left out.
FORCES = (
    ("Caldari Police", 500001, "chatCaldariStatePoliceWarning"),
    ("Minmatar Security", 500002, "chatMinmatarRepublicPoliceWarning"),
    ("Amarr Surveillance", 500003, "chatAmarrEmpirePoliceWarning"),
    ("Gallente Police", 500004, "chatGallenteFederationPoliceWarning"),
    ("CONCORD SWAT", 500006, DEFAULT_WARNING),
    ("Ammatar Surveillance", 500007, DEFAULT_WARNING),
    ("Khanid Surveillance", 500008, DEFAULT_WARNING),
)


#: CONCORD's own ships by exact type name -> role [V-data names]. Writes
#: `npc/concord.tsv` (`role`, `type_id`); `concord.rs` holds the role names
#: and no type id. A role with no type is left out, and the server flies an
#: invented fixture for it.
ROLES = (
    ("CONCORD Police Commander", "executioner"),
    ("CONCORD Police Captain", "captain"),
    ("CONCORD Police Officer", "officer"),
    ("DED Special Operation Officer", "special_frigate"),
    ("DED Special Operation Commander", "special_battleship"),
    ("CONCORD SWAT Officer", "swat_frigate"),
    ("CONCORD SWAT Commander", "swat_battleship"),
)

HEADER_ROLES = ("role", "type_id")


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def derive(type_rows, forces=FORCES):
    """`npc/police.tsv` rows from `types/typedata.tsv` data rows (split on TAB:
    `type`, id, group, ..., name last). Sorted by faction; a force with no
    type gets no row."""
    found = {}
    for p in type_rows:
        if len(p) < 4 or p[0] != "type" or int(p[2]) != GROUP:
            continue
        for prefix, faction, _warning in forces:
            if p[-1].startswith(prefix):
                found.setdefault(faction, []).append(int(p[1]))
                break
    warning = {f: w for _p, f, w in forces}
    return [(f, warning[f], ",".join(map(str, sorted(ids))))
            for f, ids in sorted(found.items())]


def derive_roles(type_rows, roles=ROLES):
    """`npc/concord.tsv` rows from `types/typedata.tsv` data rows: the type
    whose name is exactly a role's (the lowest id if two share it), in the
    order of `roles`."""
    by_name = {}
    for p in type_rows:
        if len(p) >= 4 and p[0] == "type":
            by_name.setdefault(p[-1], []).append(int(p[1]))
    return [(role, min(by_name[name])) for name, role in roles if name in by_name]


def _derive(ctx):
    return derive(list(_content().read_rows(ctx.path("types/typedata.tsv"))))


def section(ctx):
    type_rows = list(_content().read_rows(ctx.path("types/typedata.tsv")))
    rows = derive(type_rows)
    roles = derive_roles(type_rows)
    ctx.note("police: %d forces, %d types, %d concord roles"
             % (len(rows), sum(len(r[2].split(",")) for r in rows), len(roles)))
    src = [_content().fsdbin("types")]
    return [("npc/police.tsv", HEADER, rows, src),
            ("npc/concord.tsv", HEADER_ROLES, roles, src)]


def check(ctx):
    path = ctx.path("npc/police.tsv")
    if not os.path.exists(path):
        return ["npc/police.tsv: missing from the bundle"]
    bad = []
    got = [tuple(r) for r in _content().read_rows(path)]
    want = [tuple(map(str, r)) for r in _derive(ctx)]
    if got != want:
        bad.append("npc/police.tsv: differs from a fresh derive (%d vs %d rows)"
                   % (len(got), len(want)))
    wanted = {f for _p, f, _w in FORCES}
    for f in sorted(wanted - {int(r[0]) for r in got}):
        bad.append("npc/police.tsv: no row for faction %d" % f)
    fpath = ctx.path("npc/factions.tsv")
    if os.path.exists(fpath):
        known = {int(r[0]) for r in _content().read_rows(fpath)}
        for r in got:
            if int(r[0]) != 500006 and int(r[0]) not in known:
                bad.append("npc/police.tsv: faction %s is not in npc/factions.tsv" % r[0])
    return bad
