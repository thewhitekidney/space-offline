#!/usr/bin/env python3
"""Extract every ship, module, charge, entity, drone, commodity, asteroid, station,
material, blueprint, implant and wormhole type into a TSV.

The server's type knowledge used to be hand-copied rows: ~20 hulls
(``destiny::hull``), ~20 modules (``modules::TYPES``), the fitting numbers per
hull (``fitting::HULLS``) and the item rows ``/createitem`` can grant
(``item::SPAWNABLE``). Those rows stay, as the fallback for a server started
without an extract; **this file is the source when there is one.** It is CCP's
data, so -- like ``tools/celestials.py`` and ``tools/pi_statics.py`` -- this is
a *reader* and the output goes to ``out/`` (gitignored), never into the
repository. The server loads it with ``--typedata FILE``
(``server/crates/eve-server/src/typedata.rs``).

Usage (inside the analysis container)::

    tools/analysis.sh python3 /work/tools/typedata.py \\
        --client /client --resfiles /client-root/ResFiles \\
        --out /work/out/typedata.tsv

Every table is read through ``tools/dogma.py`` (layouts from the client's own
loader ``.pyd`` files, ``docs/re/dogma.md``) and ``tools/fsd.py`` (``groups``).

Format, one record per line, tab-separated; ``#`` starts a comment::

    type       typeID groupID categoryID published portionSize volume capacity
               mass radius defaultEffectID effectIDs attrs name
    attribute  attributeID defaultValue name
    effect     effectID category isOffensive durationAttributeID
               dischargeAttributeID rangeAttributeID falloffAttributeID
               trackingSpeedAttributeID name guid isWarpSafe
               fittingUsageChanceAttributeID
    attrmeta   attributeID stackable highIsGood maxAttributeID
               minAttributeID dataType
    modifier   effectID domain func modifiedAttributeID
               modifyingAttributeID operation skillTypeID groupID

``attrmeta`` and ``modifier`` are the dogma engine's input
(``server/crates/eve-server/src/dogma.rs``): the columns of
``dogmaattributes`` that decide how a modifier lands (``stackable`` picks the
stacking-penalised attribute class in ``itemAttributeFactory``,
``maxAttributeID``/``minAttributeID`` clamp it, ``dataType`` 10-12 truncates
it -- ``dogma/attributes/attribute.pyc`` ``Update``) and one line per element
of an effect's ``modifierInfo``, in the table's own order. An absent
``modifierInfo`` field (its presence bit clear) is written as an empty
column, never as ``0``: ``operation`` really is ``-1`` (``dgmAssPreAssignment``)
on 18 elements. Category 16 (*Skill*) types are written as ``type`` rows too,
because a skill's level reaches a ship only through its effects; the server
keeps them out of the item lookups.

``attrs`` is ``attributeID=value`` joined with ``;``: exactly the attributes
the type's ``typedogma`` row carries -- **presence is information** (a module
with ``chargeGroup1`` takes charges; one without does not), so nothing is
defaulted into it. The ``attribute`` records carry every attribute's own
``defaultValue`` instead, which is what the client's ``GetTypeAttribute``
returns for a type without a row (a hull that resists nothing carries no
resonance row, and the default is 1.0); the server applies it at lookup.
``effectIDs`` is a comma list; ``defaultEffectID`` is ``0`` when the type has
none (a passive module). An absent effect column is ``0``. ``radius`` is
written ``or 1.0``, the client's own ``evetypes.GetRadius``. Values are
written with ``repr``, so a float32-widened value reaches the server bit for
bit. ``name`` is last because it is the only free-text column; ``guid`` may be
empty.

``--check`` re-extracts and verifies the result instead of trusting it
(``problems``): at least one *Subsystem* (category 32) type, every one of
them carrying ``subSystemSlot`` (1366) in the four live flags and
``fitsToShipType`` (1380), every ``--check`` count printed. With ``--out`` it
also compares the file on disk with the fresh extract byte for byte. Needs
the client like the extract does; the pure part, ``problems``, is what
``tools/test_typedata.py`` runs on synthetic text.

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

#: ``categoryShip`` 6, ``categoryModule`` 7, ``categoryCharge`` 8,
#: ``categoryEntity`` 11, ``categorySkill`` 16, ``categoryCommodity`` 17,
#: ``categoryDrone`` 18, ``categoryAsteroid`` 25
#: (``inventorycommon/const.pyc``) **[V]**. Entities are the NPCs:
#: ``crate::npc::derive`` builds a belt rat's hull, gun and bounty from its row.
#: Skills are here for their effects (the dogma engine), not as items.
#: Commodities (17: tags, salvage, trade goods; C6 R7) and asteroids (25: ore
#: and its compressed forms, C1 §2.3) are stackable items, added by CP12 so
#: loot and mined ore are grantable types.
#:
#: FX7 (W17) adds, all **[V]** ``inventorycommon/const.pyc``:
#: ``categoryStation`` 3 (a station type's ``radius``, which the server's
#: station ball reads instead of one constant), ``categoryMaterial`` 4
#: (minerals; attribute 790 on them decides the reprocessing skill of the
#: randomized types, ``docs/port-tracker.md`` §72), ``categoryBlueprint`` 9
#: (so a bundle blueprint can exist as an item) and ``categoryImplant`` 20
#: (attribute 379, the same §72 reprocessing rule; and implants as items).
#:
#: HK3 adds ``categoryAncientRelic`` 34 and ``categoryDecryptors`` 35 **[V]**
#: ``inventorycommon/const.pyc``: a hacked relic or data can holds them
#: (``explore/loot.tsv``), so they must be types the store can mint.
#:
#: T3-A adds ``categorySubSystem`` 32 **[V]** ``inventorycommon/const.pyc``: the
#: 48 Tech 3 subsystems (groups 954 Defensive, 956 Offensive, 957 Propulsion,
#: 958 Core, plus the unpublished deprecated group 955). They are ingredients
#: and products of the T3 production chain (``R-T3.md`` §1.5: a walk over
#: every T3 product and material finds exactly these types missing), items the
#: store must mint, and what a modular hull's slots come from
#: (``subSystemSlot`` 1366, ``hiSlotModifier`` 1374 ...).
#:
#: FT-A adds ``categoryFighter`` 87 **[V]** ``inventorycommon/const.py:329``
#: (R-DEEP §2.1): 99 types (94 published; five ``*_PLACEHOLDER`` are not), their
#: attributes and the 13 ``fighterAbility*`` effects, for the carriers' fighter
#: bay and squadrons. Class is a flag attribute (2212-2214, standup 2740-2742),
#: not the group.
#: U0 adds ``categoryStructure`` 65 (Astrahus, Raitaru, Athanor ... : groups 1657, 1404, 1406)
#: and ``categoryStructureModule`` 66 (Standup service modules, fuel group 1136)
#: **[V-data]** (r-upwell §4): player-owned structures and what fits them.

#:
#: NW-2 adds ``categoryDeployable`` 22 **[V]** (``inventorycommon/const.py``,
#: r-null-war §2): the Mobile Warp Disruptors (group 361, effects anchorDrop
#: 649 / anchorLift 650, attributes 103 radius, 556 delay and 9 hull points),
#: the mobile cyno inhibitor (1249) and the rest of the category. Groups 1246
#: and 1250 were already listed by group. **The bundle needs the typedata
#: section regenerated to carry them** (``tools/content.py`` with the
#: ``typedata`` section into a NEW bundle, never ``--sections`` on a live one).
CATEGORIES = (3, 4, 6, 7, 8, 9, 11, 16, 17, 18, 20, 22, 25, 32, 34, 35, 65, 66, 87)

#: ``categoryFighter`` **[V]**, and the flag attributes of a fighter's class:
#: ``fighterSquadronIsLight/Support/Heavy`` 2212-2214 and the standup
#: variants 2740-2742 (R-DEEP §2.1). A published fighter has exactly one set.
CATEGORY_FIGHTER = 87
ATTR_FIGHTER_CLASS_FLAGS = (2212, 2213, 2214, 2740, 2741, 2742)
#: ``fighterSquadronMaxSize`` 2215: every published fighter carries it.
ATTR_FIGHTER_SQUADRON_MAX_SIZE = 2215

#: ``categorySubSystem`` **[V]**, and the attributes a subsystem must carry:
#: ``subSystemSlot`` (the flag it is fitted in), ``fitsToShipType`` (its hull).
CATEGORY_SUBSYSTEM = 32
ATTR_SUBSYSTEM_SLOT = 1366
ATTR_FITS_TO_SHIP_TYPE = 1380
#: ``flagSubSystemSlot0..3`` **[V]**: the four live (visible) subsystem flags.
SUBSYSTEM_FLAGS = (125, 126, 127, 128)
#: The attributes that give a hull its slots and hardpoints
#: (``hiSlotModifier``, ``medSlotModifier``, ``lowSlotModifier``,
#: ``turretHardPointModifier``, ``launcherHardPointModifier``).
ATTR_SLOT_MODIFIERS = (1374, 1375, 1376, 1368, 1369)

#: Single groups taken from a category that is otherwise left out (FX7):
#: ``groupWormhole`` 988 (category 2, *Celestial*) **[V]**, for its
#: ``wormholeMaxStableTime``/``MaxStableMass``/``MaxJumpMass`` and target
#: class (``server/.../wormhole.rs`` ``typedata_facts``), and
#: ``groupHarvestableCloud`` 711 (category 2) **[V]**, the gas clouds: the
#: cloud's type is the gas a harvester puts in the hold (SOC-B2,
#: ``server/.../mining.rs``). The rest of category 2 (containers, beacons,
#: wrecks...) stays out: nothing reads it, and it would be thousands of rows.
#: D-HUB adds ``groupInfrastructureHub`` 1012 (category 40, *Sovereignty
#: Structure*) **[V-data]**: the Sovereignty Hub's own type row was missing
#: (its blueprint 2735 is in the bundle, its product 32458 was not), so the
#: server's item store could not create one and no job could make it.
#: U0: ``groupStructureDeed`` 4086 **[A]** (Astrahus's ``structureRequiresDeedType``
#: 56201 sits in it, r-upwell §4); falsifier: type 56201 absent from the types section
#: after regeneration. Its category is unconfirmed, so the group is named here.
#: #57 CONTAINERS: ``groupCargoContainer`` 12, ``groupSecureCargoContainer`` 340,
#: ``groupAuditLogSecureContainer`` 448 and ``groupFreightContainer`` 649
#: (``invConst.playerDeployedContainers``) **[V]**, all category 2: the types a
#: pilot assembles, fills and password-protects in a station. They stack
#: packaged and are items (``server/.../typedata.rs`` lets these groups through
#: the celestial filter); the server's hand rows know only type 23.
#: #82 MTU-DEPOT: ``groupMobileHomes`` 1246 (the Mobile Depot) and
#: ``groupAutoLooter`` 1250 (the Mobile Tractor Unit) **[V]**
#: (``inventorycommon/const.py``; both in ``invControllers.LOOT_GROUPS``), in
#: ``categoryDeployable`` 22. Their blueprints (groups 1269, 1270) were already
#: in the bundle; the units themselves were not, so nothing could grant, launch
#: or list one. The server's component figures for them are
#: ``server/.../mtu.rs`` ``spec`` (read from ``spacecomponentsbytype``).
#: FIX-W6D: the crate groups of ``mining/geode-candidates.tsv`` (``groupMiningCrates``
#: 1194 and 1818, 1886, 1977, 314, 526): ``wanted()`` skipped them, so the ORE caches
#: had no typedata row and ``Edit::Create`` said UnknownType. By group, because the
#: candidates file is the owner's list and grows. The bundle needs a full regen.
#: SYS-1: ``groupEffectBeacon`` 920 (category 2) **[V-data]** (r-wormhole §3): the
#: beacons of ``universe/system-effects.tsv``, with their category 7 effects and
#: modifiers (the ``used_effects`` walk follows each type's ``effectIDs``). Not
#: items: ``typedata.rs`` keeps category 2 out of the item lookups.
GROUPS = (12, 314, 340, 448, 526, 649, 711, 920, 988, 1012, 1194, 1246, 1250, 1818, 1886,
          1977, 4086)


def wanted(group_id, category_id):
    """Whether a type of this group and category is extracted."""
    return category_id in CATEGORIES or group_id in GROUPS


def _opt(v):
    """An optional integer column: empty when the field is absent."""
    return "" if v is None else str(int(v))

def _num(v):
    """``repr`` of a float, plain digits for an integral one."""
    if isinstance(v, bool):
        return "1" if v else "0"
    if isinstance(v, float):
        return repr(v)
    return str(int(v))


def _clean(s):
    return str(s).replace("\t", " ").replace("\n", " ").replace("\r", " ")


def type_line(t, row, cat, attrs, effects, default, name):
    """One ``type`` record. Pure, so a test can feed it invented bytes.

    ``row`` is the ``types`` row (``groupID``, ``published`` ...), ``attrs``
    ``{attributeID: float}``, ``effects`` the effect ids."""
    return "\t".join([
        "type", str(t), str(row["groupID"]), str(cat),
        "1" if row.get("published") else "0",
        str(int(row.get("portionSize") or 1)),
        _num(float(row.get("volume") or 0.0)),
        _num(float(row.get("capacity") or 0.0)),
        _num(float(row.get("mass") or 0.0)),
        _num(float(row.get("radius") or 1.0)),
        str(default),
        ",".join(str(e) for e in effects),
        ";".join("%d=%s" % (i, _num(v)) for i, v in sorted(attrs.items())),
        _clean(name),
    ])


def effect_line(e, r):
    """One ``effect`` record from a dogmaeffects row (a dict)."""
    return "\t".join([
        "effect", str(e), str(r.get("effectCategory") or 0),
        "1" if r.get("isOffensive") else "0",
        str(r.get("durationAttributeID") or 0),
        str(r.get("dischargeAttributeID") or 0),
        str(r.get("rangeAttributeID") or 0),
        str(r.get("falloffAttributeID") or 0),
        str(r.get("trackingSpeedAttributeID") or 0),
        _clean(r.get("effectName") or ""),
        _clean(r.get("guid") or ""),
        "1" if r.get("isWarpSafe") else "0",
        str(r.get("fittingUsageChanceAttributeID") or 0),
    ])


def problems(text):
    """What is wrong with an extract, as a list of strings (empty: fine).

    The subsystem rule (T3-A): category 32 must be there, and each
    *published* one must say which flag it is fitted in (one of the four live
    ones), which hull it fits, and carry at least one slot or hardpoint
    modifier (that is how a modular hull gets its slots). The deprecated
    unpublished group is extracted and not held to this.
    """
    bad = []
    subsystems = []
    for line in text.splitlines():
        f = line.split("\t")
        if f[0] != "type" or len(f) < 14:
            continue
        if int(f[3]) != CATEGORY_SUBSYSTEM:
            continue
        attrs = {}
        for pair in filter(None, f[12].split(";")):
            k, _, v = pair.partition("=")
            attrs[int(k)] = float(v)
        subsystems.append((int(f[1]), f[4] == "1", attrs, f[13]))
    if not subsystems:
        bad.append("no category %d (subsystem) type was extracted" % CATEGORY_SUBSYSTEM)
    bad += fighter_problems(text)
    for t, published, attrs, name in subsystems:
        if not published:
            continue
        if int(attrs.get(ATTR_SUBSYSTEM_SLOT, 0)) not in SUBSYSTEM_FLAGS:
            bad.append("subsystem %d (%s): subSystemSlot %s is not one of %s"
                       % (t, name, attrs.get(ATTR_SUBSYSTEM_SLOT), SUBSYSTEM_FLAGS))
        if not attrs.get(ATTR_FITS_TO_SHIP_TYPE):
            bad.append("subsystem %d (%s): no fitsToShipType" % (t, name))
        if not any(a in attrs for a in ATTR_SLOT_MODIFIERS):
            bad.append("subsystem %d (%s): published and no slot modifier attribute"
                       % (t, name))
    return bad


def fighter_problems(text, require=False):
    """The fighter rule (FT-A): category 87 must be there, and each *published*
    fighter carries exactly one class flag set to 1 and a squadron size > 0.
    With ``require`` (the real ``--check``) a text with no fighter at all fails;
    ``problems`` on a small synthetic text does not demand them."""
    bad = []
    seen = 0
    for line in text.splitlines():
        f = line.split("\t")
        if f[0] != "type" or len(f) < 14:
            continue
        if int(f[3]) != CATEGORY_FIGHTER:
            continue
        seen += 1
        if f[4] != "1":
            continue
        attrs = {}
        for pair in filter(None, f[12].split(";")):
            k, _, v = pair.partition("=")
            attrs[int(k)] = float(v)
        flags = [a for a in ATTR_FIGHTER_CLASS_FLAGS if attrs.get(a) == 1.0]
        if len(flags) != 1:
            bad.append("fighter %s (%s): %d class flags set (want exactly one of %s)"
                       % (f[1], f[13], len(flags), ATTR_FIGHTER_CLASS_FLAGS))
        if not attrs.get(ATTR_FIGHTER_SQUADRON_MAX_SIZE, 0) > 0:
            bad.append("fighter %s (%s): no fighterSquadronMaxSize" % (f[1], f[13]))
    if require and not seen:
        bad.append("no category %d (fighter) type was extracted" % CATEGORY_FIGHTER)
    return bad


def usage_chance_problems(text, require=False):
    """The booster rule (B0): the side effects are the effects with a
    fittingUsageChanceAttributeID. With ``require`` (the real ``--check``) an
    extract with none fails, because the server would then apply every side
    effect for good."""
    seen = False
    for line in text.splitlines():
        f = line.split("\t")
        if f[0] == "effect" and len(f) > 12 and f[12] not in ("", "0"):
            seen = True
            break
    if require and not seen:
        return ["no effect carries a fittingUsageChanceAttributeID"]
    return []


def extract(client, resfiles):
    d = dogma.Dogma(client, resfiles)
    c = d.client
    groups = fsd.cfsd_rows(
        fsd.CfsdTable(c.path("res:/staticdata/groups.fsdbinary"), "groups"), "groups")
    names = d.type_names()
    defaults = {k: float(v.get("defaultValue") or 0.0) for k, v in d.attributes.items()}

    types = {}
    for key, row in d.table("types").rows():
        cat = groups.get(row["groupID"], {}).get("categoryID")
        if wanted(row["groupID"], cat):
            types[key] = (row, cat)

    dogma_rows = {}
    for key, row in d.table("typedogma").rows():
        if key in types:
            dogma_rows[key] = row

    out = []
    out.append("# eve-offline type data, written by tools/typedata.py from the")
    out.append("# player's own client install. CCP's data: never commit this file.")
    used_effects = set()
    counts = {}
    for t in sorted(types):
        row, cat = types[t]
        dr = dogma_rows.get(t, {"dogmaAttributes": [], "dogmaEffects": []})
        attrs = {e["attributeID"]: float(e["value"]) for e in dr["dogmaAttributes"]}
        effects = [e["effectID"] for e in dr["dogmaEffects"]]
        default = next((e["effectID"] for e in dr["dogmaEffects"] if e["isDefault"]), 0)
        used_effects.update(effects)
        name = names.get(t)
        name = name if isinstance(name, str) else "type %d" % t
        out.append(type_line(t, row, cat, attrs, effects, default, name))
        counts[cat] = counts.get(cat, 0) + 1

    for a in sorted(defaults):
        out.append("\t".join([
            "attribute", str(a), _num(defaults[a]),
            _clean(d.attributes[a].get("name") or ""),
        ]))
    for a in sorted(defaults):
        r = d.attributes[a]
        out.append("\t".join([
            "attrmeta", str(a),
            "1" if r.get("stackable") else "0",
            "1" if r.get("highIsGood") else "0",
            str(int(r.get("maxAttributeID") or 0)),
            str(int(r.get("minAttributeID") or 0)),
            str(int(r.get("dataType") or 0)),
        ]))

    effs = d.effects
    n_effects = 0
    for e in sorted(used_effects):
        r = effs.get(e)
        if r is None:
            continue
        out.append(effect_line(e, r))
        for m in r.get("modifierInfo") or []:
            out.append("\t".join([
                "modifier", str(e),
                _clean(m.get("domain") or ""),
                _clean(m.get("func") or ""),
                _opt(m.get("modifiedAttributeID")),
                _opt(m.get("modifyingAttributeID")),
                _opt(m.get("operation")),
                _opt(m.get("skillTypeID")),
                _opt(m.get("groupID")),
            ]))
        n_effects += 1
    return "\n".join(out) + "\n", counts, n_effects


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--client", default=os.environ.get("EVE_CLIENT"))
    p.add_argument("--resfiles", default=None)
    p.add_argument("--out", default=None)
    p.add_argument("--check", action="store_true",
                   help="verify a fresh extract (and --out's file, if given) "
                        "instead of writing")
    a = p.parse_args(argv)
    if not a.check and not a.out:
        p.error("--out is required unless --check")
    text, counts, n_effects = extract(a.client, a.resfiles)
    if a.check:
        bad = (problems(text) + fighter_problems(text, require=True)
               + usage_chance_problems(text, require=True))
        bad = list(dict.fromkeys(bad))
        if a.out:
            try:
                with open(a.out, encoding="utf-8", newline="") as fh:
                    on_disk = fh.read()
            except OSError as e:
                bad.append("%s: %s" % (a.out, e))
            else:
                if on_disk != text:
                    bad.append("%s differs from a fresh extract (%d vs %d bytes)"
                               % (a.out, len(on_disk), len(text)))
        print("typedata --check: %d subsystems (category 32)" % counts.get(32, 0),
              file=sys.stderr)
        for b in bad:
            print("typedata --check: " + b, file=sys.stderr)
        return 1 if bad else 0
    with open(a.out, "w", encoding="utf-8") as f:
        f.write(text)
    print("typedata: %d ships, %d modules, %d charges, %d entities, %d skills, %d drones, "
          "%d commodities, %d asteroids, %d stations, %d materials, %d blueprints, "
          "%d implants, %d subsystems, %d wormholes, %d effects -> %s"
          % (counts.get(6, 0), counts.get(7, 0), counts.get(8, 0), counts.get(11, 0),
             counts.get(16, 0), counts.get(18, 0), counts.get(17, 0), counts.get(25, 0),
             counts.get(3, 0), counts.get(4, 0), counts.get(9, 0), counts.get(20, 0),
             counts.get(32, 0), counts.get(2, 0), n_effects, a.out), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
