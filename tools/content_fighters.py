"""The `fighters` content section: carrier fighters' abilities and slots.

Packet FT-A (`docs/lanes/ft-a.md`; spec R-DEEP §2.3, §2.8). Writes two bundle
files, every header `docs/content/schema.md`'s:

    fighters/abilities.tsv   one row per ability (`fighterabilities.static`):
                             target mode, sec-space bans, the dogma effect
                             that carries its numbers, icon, turret, name
    fighters/slots.tsv       one row per populated ability slot of a fighter
                             type (`fighterabilitiesbytype.static`): which
                             ability, cooldown, charges and rearm time

Both tables are the client's own fsdlite sqlite caches, read with
`fsd.sqlite_rows`. `fighterabilities.static` has **no** link to a dogma effect:
the 36 -> 13 map is the client's `dogmaEffectByAbilityID`
[V `fighters/abilityAttributes.py`], copied here as `ABILITY_EFFECT` and
checked against the extracted typedata's effect rows. A fighter's class
(light, support, heavy), squadron size, role and refuel time are *attributes*
of its type row (2212-2215, 2740-2742, 2270, 2426), not columns here.

The derivation is pure (plain dicts in, rows out): the unit tests feed it
invented rows; `gather(ctx)` is the only bundle reader.
"""

import os
import sys

SECTION = "fighters"
DEPENDS = ("types",)

_HERE = os.path.dirname(os.path.abspath(__file__))

ABILITIES_RES = "res:/staticdata/fighterabilities.static"
SLOTS_RES = "res:/staticdata/fighterabilitiesbytype.static"

# The header lines, copied from `schema.md`.
ABILITIES_HEADER = tuple(
    "ability_id target_mode low_sec_banned high_sec_banned effect_id icon_id "
    "turret_graphic_id name".split())
SLOTS_HEADER = tuple("type_id slot ability_id cooldown_s charges rearm_s".split())

TARGET_MODES = ("untargeted", "itemTargeted", "pointTargeted")
#: `categoryFighter` [V `inventorycommon/const.py:329`].
CATEGORY_FIGHTER = 87

#: `dogmaEffectByAbilityID` [V `fighters/abilityAttributes.py`]: the effect
#: that carries each ability's numbers (R-DEEP §2.3). Abilities 1, 3 and 6 are
#: used by no shipped type, so their effects (6430, 6419, 6420) need not be in
#: the extracted typedata; every other effect must.
ABILITY_EFFECT = {
    # attack turret / cannon / "attack missile": fighterAbilityAttackM
    **{a: 6465 for a in (17, 22, 23, 24, 25, 26, 27, 44, 45)},
    # missile, micro missile swarm, heavy rocket salvo, torpedo salvo
    **{a: 6431 for a in (14, 29, 30, 31, 32, 33, 34, 35, 36, 18, 19, 20, 21)},
    2: 6435,    # stasis webifier
    10: 6436,   # warp disruption
    11: 6434,   # energy neutralizer
    12: 6437,   # ECM
    4: 6441,    # microwarpdrive
    9: 6440,    # afterburner
    5: 6442,    # micro jump drive
    13: 6439,   # evasive maneuvers
    7: 6485,    # launch bomb
    16: 6464,   # tackle
    38: 6554,   # kamikaze
    1: 6430,    # attack turret (unused)
    3: 6419,    # target painter (unused)
    6: 6420,    # damage boost (unused)
}
#: Abilities no shipped type uses: their effect rows may be absent.
UNUSED_ABILITIES = (1, 3, 6)


def _opt(v):
    return "" if v is None else v


def abilities_rows(abilities, text):
    """`fighters/abilities.tsv` rows, ascending by ability id. `abilities` is
    `{id: row}` from `fighterabilities.static`; `text` maps a message id to
    its name. An ability with no `ABILITY_EFFECT` entry gets effect 0, which
    `check` then refuses."""
    rows = []
    for a, r in sorted(abilities.items()):
        rows.append((a, r.get("targetMode", ""),
                     1 if r.get("disallowInLowSec") else 0,
                     1 if r.get("disallowInHighSec") else 0,
                     ABILITY_EFFECT.get(a, 0), r.get("iconID") or 0,
                     _opt(r.get("turretGraphicID")),
                     text(r.get("displayNameID"))))
    return rows


def slots_rows(by_type):
    """`fighters/slots.tsv` rows, ascending by `(type, slot)`; a slot the type
    does not populate has no row, and an absent cooldown or charges is an
    empty column."""
    rows = []
    for t, r in sorted(by_type.items()):
        for slot in range(3):
            s = r.get("abilitySlot%d" % slot)
            if not s:
                continue
            ch = s.get("charges") or {}
            rows.append((t, slot, s["abilityID"], _opt(s.get("cooldownSeconds")),
                         _opt(ch.get("chargeCount")), _opt(ch.get("rearmTimeSeconds"))))
    return rows


# --- the bundle readers ---------------------------------------------------------

def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def gather(ctx):
    if "fighters" in ctx._cache:  # noqa: SLF001
        return ctx._cache["fighters"]  # noqa: SLF001
    fsd = _content()._imp("fsd")  # noqa: SLF001
    c = ctx.client
    got = ({int(k): v for k, v in fsd.sqlite_rows(c.path(ABILITIES_RES))},
           {int(k): v for k, v in fsd.sqlite_rows(c.path(SLOTS_RES))})
    ctx._cache["fighters"] = got  # noqa: SLF001
    return got


def derive(abilities, by_type, text):
    return abilities_rows(abilities, text), slots_rows(by_type)


def section(ctx):
    abilities, by_type = gather(ctx)
    ab, sl = derive(abilities, by_type, ctx.text)
    ctx.note("fighters: %d abilities, %d slots over %d fighter types"
             % (len(ab), len(sl), len({r[0] for r in sl})))
    return [("fighters/abilities.tsv", ABILITIES_HEADER, ab, [ABILITIES_RES]),
            ("fighters/slots.tsv", SLOTS_HEADER, sl, [SLOTS_RES])]


# --- the checks -------------------------------------------------------------------

def problems(ab, sl, effects, fighters):
    """What is wrong with the two row lists, as strings. `effects` is the set
    of effect ids typedata extracted; `fighters` the published category-87 type
    ids. Pure, so the tests run it on invented rows."""
    bad = []
    ids = {r[0] for r in ab}
    for r in ab:
        if r[1] not in TARGET_MODES:
            bad.append("abilities.tsv: ability %d: target_mode %r" % (r[0], r[1]))
        if not r[4]:
            bad.append("abilities.tsv: ability %d has no effect in ABILITY_EFFECT" % r[0])
    used = {r[2] for r in sl}
    for a in sorted(used - ids):
        bad.append("slots.tsv: ability %d is not in abilities.tsv" % a)
    for r in ab:
        if r[0] in used and r[4] and r[4] not in effects:
            bad.append("abilities.tsv: ability %d: effect %d is not in typedata" % (r[0], r[4]))
    seen = set()
    for r in sl:
        if (r[0], r[1]) in seen:
            bad.append("slots.tsv: (%d, %d) twice" % (r[0], r[1]))
        seen.add((r[0], r[1]))
        if r[0] not in fighters:
            bad.append("slots.tsv: type %d is not a published category-87 type" % r[0])
    return bad


def check(ctx):
    content = _content()
    typedata = _imp_typedata()
    abilities, by_type = gather(ctx)
    ab, sl = derive(abilities, by_type, ctx.text)
    bad = []
    for name, rows in (("abilities", ab), ("slots", sl)):
        got = list(content.read_rows(ctx.path("fighters/%s.tsv" % name)))
        if got != [content.encode_row(r).split("\t") for r in rows]:
            bad.append("fighters/%s.tsv: differs from a fresh derive (%d vs %d rows)"
                       % (name, len(got), len(rows)))
    effects, fighters = set(), set()
    text = []
    for line in open(ctx.path("types/typedata.tsv"), encoding="utf-8", newline=""):
        text.append(line)
        f = line.rstrip("\n").split("\t")
        if f[0] == "effect":
            effects.add(int(f[1]))
        elif f[0] == "type" and len(f) >= 14 and int(f[3]) == CATEGORY_FIGHTER and f[4] == "1":
            fighters.add(int(f[1]))
    bad += problems(ab, sl, effects, fighters)
    bad += typedata.fighter_problems("".join(text), require=True)
    missing = sorted(fighters - {r[0] for r in sl})
    if missing:
        bad.append("slots.tsv: %d published fighters have no slot (first %d)"
                   % (len(missing), missing[0]))
    ctx.note("fighters: %d abilities, %d slots, %d published fighters in typedata"
             % (len(ab), len(sl), len(fighters)))
    return bad


def _imp_typedata():
    return _content()._imp("typedata")  # noqa: SLF001
