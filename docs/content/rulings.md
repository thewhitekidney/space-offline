# Coordinator rulings on the content-plan review (2026-10-25)

Input: `plan.md` (S1) and `plan-review.md` (R1). These rulings bind the
revision lane, which writes plan v2 into `plan.md`. Owner decisions stay
open; none of these makes one.

1. **Extractor tests:** accepted.
   - Done: `tools/gate.sh` now runs every `tools/test_*.py`.
   - Every extractor packet ships synthetic-byte unit tests in that form.
   - Counts that need the client ("260,242 objects") belong in
     `content.py --check`, which `up.sh` runs before boot, never in the gate.
   - The plan's claim that the gate checks counts is struck.
2. **Seams S6-S12:** accepted. B1 has landed, so all seams land before
   W11, as a W10.5 coordinator wave. The Registry, the ticker stage list,
   warp resolution, `scan.rs`, slim items and the mission hooks get
   extension points: one registration line per subsystem, the same pattern
   as `notify.rs` and `DOMAINS`.
3. **How code reads loaded data:** a `Content` value owned by the world and
   reached through the Registry, so tests build their own.
   - It replaces process-wide `OnceLock` tables.
   - Tests build `Content::fixture()` with invented rows.
   - The server without a bundle runs on the fixture, which keeps every wire
     test working, and logs a loud warning.
   - O3 then means the bundle is needed to *play*, never to *test*.
   - CP32 and CP33 are split along the review's lines, and CP33 includes
     `service/industry.rs`.
4. **Delta keys come from client ids that stay stable, never from row
   position.**
   - A rock is keyed by (belt itemID, the rock's ordinal within that belt's
     own rows from the client), and its type and position are checked when
     it loads; on a mismatch the delta is dropped.
   - A site slot is keyed by (system, archetype, `population.tsv` row key),
     never by row index.
5. **End-goal gaps:** accepted as new packets in the waves where their
   dependencies are met:
   - wormholes (statics, K162, mass and decay; authored statics, since the
     client ships none);
   - full `facWarMgr` plus victory points, enlistment and FW sites;
   - system security fed into crimewatch;
   - warp disruption and scrambling (tackle);
   - a bot role that runs sites, and a bot role that fights humans, opt-in
     per bot.
   - The CP39 courtesy rule only applies to bots set as "civilian".
6. **CP2 stations:** the test target stays at 5,210, and the extractor also
   takes stations listed under the star.
7. **Downtime:** the server rolls over at 11:00 UTC. The jail sets `TZ=UTC`
   for our own clients. A friend's client in another time zone shows a
   different reset countdown, which is only cosmetic. Record it as a known
   limitation, and add a human check.
8. **No [A] value in any test** (CP35, CP39, CP40): rewrite those tests to
   assert the rule, not the number.
9. **`rewards.tsv`:** no EvEJS rows. We author our own values from the
   client's own data (bounties, `basePrice`, mission levels). EvEJS is
   facts and vocabulary only, as it always was, so this needs no owner
   decision and O8 is dropped.

## Errata found while landing W10.5 (2026-10-25)

10. **Pinning test names:** the plan cited `domains_are_disjoint`, which
    doesn't exist. The real test is
    `service::tests::no_service_is_claimed_by_two_modules`, and `plan.md`
    is corrected.
11. **CP26b runs after CP22** (X1, `schema.md`). Dungeon entity ids are not
    type ids: 5,777 of 30,586 coincide only by chance. A bounty-based reward
    base therefore needs CP22's squad fill first.
12. **CP28a's slim extra `objectiveTargetGroup` has no data source**: it is
    set on 0 objects and 0 entities. CP28a drops that field, or derives it
    from the objective tables. The lane decides, and says which.
13. **Mission LP (X1):** the client keeps separate Alpha and Omega fixed-LP
    values. `fixed_lp` uses the Omega value, marked [A].
14. **Mission families (CP19):** the family-uniform draw keys on
    `chain_key`, and otherwise on `name_id`. CP13's `missions.tsv` writer
    **canonicalises `name_id` per title**: every variant whose title text
    is the same gets the lowest of its `name_id`s. This needs no schema
    change. CP13's `--check` reports any title that had more than one id.
