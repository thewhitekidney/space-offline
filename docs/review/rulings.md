# Coordinator rulings on the emulator-review red team (2026-10-25)

Input: `plan.md` (SR1) and `plan-review.md` (RR1). These rulings bind the
revision lane, which writes v2 into `plan.md`. Owner decisions stay open.

1. **C1, the client build:** accepted. **O17**, how friends get build
   3619879, is a new owner decision. Recommended answer: the owner shares
   the pinned client copy privately. In accounts mode the server refuses
   every build other than `BOOT_BUILD`, not only older ones.
2. **C2, tokens:** accepted in full.
   - Each listener has a role ceiling, and a friends' listener can never
     grant GM.
   - Tokens are random, generated per account and stored outside the repo.
   - Bots read their tokens from a file.
   - Nothing is ever derived from a visible name.
3. **C3, Docker and libvirt sockets:** accepted. Every launch path drops
   supplementary groups with `setpriv --clear-groups`, never `sudo -u`, and
   the jail checks from inside that the client has no `docker`, `libvirt` or
   `sudo` group.
   - **Fact for the record:** every unattended client run on 2026-10-25 went
     through the helper, which already passes `--clear-groups`, so those runs
     never held the group.
4. **C4, the jail is an allowlist, not a denylist:** accepted.
   - The client's mount namespace gets a tmpfs over `/run` and over its
     runtime dir, binding back only display, plus audio for human play.
   - It gets a private `/dev/shm`.
   - `WINEDLLOVERRIDES=winebrowser.exe=d`.
   - No more unattended client runs until this lands. The CI half (a
     private runtime dir) is already in.
5. **H1:** IP33's wording is corrected. The jail stops the client phoning
   home. It does **not** protect a friend from a compromised server. Say so
   plainly in the friend kit.
6. **H2, H3, H4, H8:** accepted.
   - Friends connect through the rootless jail with a unix-socket relay, so
     they need no sudo.
   - The friend path lands only after accounts and the GM gate (H3).
   - sshd, if used, runs in a container. That is owner decision O18.
   - O11 (friends' platforms) is asked now, and Windows is designed or
     declared out of scope.
7. **H5:** a friend kit with an allowlist of files, never the repo.
8. **H6, H7:** character ownership is world state, decided before IP21. The
   `/ssoToken` check moves to the first wave that touches accounts, and the
   identity-by-listener fallback is designed alongside it.
9. **M1, sizes:** re-split IP36, IP37 and IP38 under the 1,500-line cap.
   IP28 is a lane packet, not a coordinator packet, and every caller it
   touches is in its ownership or in a coordinator hunk.
10. **The rest of the mediums and lows:** apply as RR1 proposes. That
    includes: the decode budget applies only to the server's inbound path,
    not the bots' decoder; a recovered panic never saves; the connection cap
    is configurable, with headroom; no 1.1.1.1 probe; backups are keyed by
    world-format version plus git sha.
11. **The rootless jail (IP29) cannot drop groups**, so ruling 3 is met by
    masking instead. An unprivileged user namespace cannot call `setgroups`
    to shed supplementary groups, and the kernel still honours an unmapped
    group in permission checks. So in that jail, every privileged socket
    is made unreachable by the mount namespace:
    - an empty tmpfs over `/run` (the allowlist from ruling 4);
    - an empty file bound over `/var/run/docker.sock` and over the libvirt
      sockets wherever they live outside `/run`.

    An in-jail check must then **fail to connect** to each of them. That
    check is a gate on the client starting, not a log line. CI may use the
    rootless jail only once that check is in place.

## Errata found while landing IW0a (2026-10-25)

12. **The account spec (IP40) adds hunks the plan rows lack.** Each lane
    brief must include them:
    - **IP21:** `service.rs` and `dispatch.rs` hunks for `Caller` and
      `Registry::dispatch_as`. Without them the GM gate cannot see the
      caller's role.
    - **IP27-a:** a `dispatch.rs` hunk, so the resolved account is copied
      into the session.
    - **IP27-b:** hunks in `server.rs`, `bin/eve-server.rs` and
      `dispatch.rs`, and the new test
      `another_accounts_character_cannot_be_selected`. Characters become a
      world record, which means a world-format bump.
    - **IP22-a:** per-key `permitopen`, only if the `/ssoToken` probe fails.
    - `docs/lanes/bots.md` §380-388 still describes `--player`. IP41
      corrects it.
13. **O14 (release by default) is the owner's decision.** Until it is
    answered, IP6 ships release as an opt-in `--release` flag, and the
    default stays debug.
14. **The jailed client shares its Wine prefix (8500) with the owner's
    online EVE install** (IP14 lane note). A compromised jailed client
    could plant DLLs that the online client later loads. New owner decision
    **O19**. Recommended: give the offline client its own prefix (a copy of
    8500), so nothing it writes ever reaches the online install.
15. **Three leaks in IP14's helper allowlist** (found by IP29's rootless
    gate; `docs/lanes/ip29.md` has a "Leaks" section):
    - `/var/snap/canonical-livepatch/.../livepatchd.sock` and
      `/var/snap/cups/common/run/cups.sock` accept connections from uid 1000
      outside the masked trees;
    - the CIFS NAS automounts under `/media/$USER/*` are readable and
      writable from inside the jail, and the host kernel carries that
      traffic outside the netns;
    - without a pid namespace of its own, `/proc/PID/root` of an unmasked
      process reaches host sockets.

    **A follow-up packet, IP14b, ports IP29's three fixes into
    `eve-offline-jail.in`:** a host socket inventory, with each socket masked
    by an empty read-only bind and a connect-gate; tmpfs over `/media` and
    `/mnt`; and a pid namespace plus `/proc` of its own for the client.
    It takes effect only on the owner's helper reinstall. **Until it lands,
    the jailed client can reach the NAS shares and those two sockets.**
    Neither reaches the internet, so the no-phone-home guarantee holds, but
    the client can touch the owner's files on the NAS.
