//! The encoders: the ball state blob and the slim items, byte for byte.
//!
//! Moved verbatim out of `destiny.rs` (IP36-b); `destiny` re-exports every
//! item here with `pub use encode::*`, so every caller's path is unchanged.
//! The module doc of [`super`] is the spec this code satisfies.

use super::warp_arrival;
use crate::pyutil::instance;
use eve_marshal::PyValue;

#[cfg(doc)]
use super::WreckSlim;

/// Packet type `0`: a full state, which is what `ReadFullStateFromStream`
/// expects.
///
/// `1` means `WriteBallsToStream` -- a ball list with no header -- and the
/// reader rejects anything `> 1` with `Unknown packet type`. **[V]**
pub const PACKET_FULL_STATE: u8 = 0;

/// Flags bit 0, `isFree`, which **gates the 36-byte physics block**.
///
/// Set: `maxVelocity`, the velocity triple, `Agility` and `speedFraction` are
/// present, and the record is 100 bytes. Clear: they are absent and the record
/// is 64. Measured on `10-arg-a5_i.bin`, a 69-byte blob (5 + 64) whose flag
/// byte is `0x1e`. **[V]**
pub const FLAG_IS_FREE: u8 = 0x01;

/// The flag byte the client's own dump carried for a normal free ball.
///
/// `0x1f`. Bits 1-4 are not decoded here: only bit 0 changes the record's
/// length, so only bit 0 has to be understood to write a correct blob. The
/// others ride through the round trip untouched, which is the honest way to
/// carry a field whose meaning we do not know.
pub const FLAGS_FREE_BALL: u8 = 0x1f;

/// Flags bit 2, `isMassive`: the client's native collides massive balls.
///
/// **The client writes it set by default** (`0x1f`), and TQ clears it on
/// ships (CAPTURE-UNDOCK, EVEJS-MOTION: EvEJS sends a ship as `0x01`/`0x09`
/// and `SetBallMassive(ship, 0)` at undock and at every warp landing). The
/// owner's capture shows the ego and every bot Hulk as `0x1f` beside a
/// station and statics of `0x1e` with mass 0.0: a massive ship at a massive
/// mass-0 station mouth. `[A]` that this is the cause of the 15 km/s and the
/// bouncing; falsified by the symptoms surviving this change.
pub const FLAG_IS_MASSIVE: u8 = 0x04;

/// A ship's ball: the client's default with [`FLAG_IS_MASSIVE`] cleared.
pub const FLAGS_SHIP_BALL: u8 = FLAGS_FREE_BALL & !FLAG_IS_MASSIVE;

/// A static ball (station, planet, rock, wreck, structure): not free, and
/// not massive.
pub const FLAGS_STATIC_BALL: u8 = FLAGS_FREE_BALL & !FLAG_IS_FREE & !FLAG_IS_MASSIVE;

/// The byte after the physics block, `0xff` in every dump.
///
/// **A field, not a terminator**: in `04-two-balls.bin` the second record
/// starts at the very next byte, and in the `isFree=0` dump it is still
/// written, immediately after `allianceID`. **[V]**
///
/// `[I]` that it is `formationID` -- the client's own readback prints
/// `formationID 255` -- and deliberately unresolved: a dump in which it
/// differs from 255 would settle it, and
/// `destiny-ref/80-call-SetBallFormation.bin` came back byte-identical to the
/// baseline with the mode unchanged, so `SetBallFormation` is not the way to
/// produce one.
pub const NO_FORMATION: u8 = 0xff;

/// A ball's mode, carrying the tail that mode writes.
///
/// **The mode code and the tail are one value on purpose.** They are written
/// 0x5b bytes apart and a disagreement between them is unreadable to the
/// client -- the native parser consumes the tail by mode, so a `FOLLOW` with
/// no tail swallows the next record's first 12 bytes and every ball after it
/// is garbage. Making them a single enum means that cannot be expressed.
///
/// Tail lengths are measured from the five dumps named below, each the same
/// two-ball park plus one call, so the tail length is the file's size
/// difference. **[V]**
///
/// **Two of the client's nine modes are still missing**, and that is
/// deliberate: `FORMATION` and `MUSHROOM` have tails that are `[V2]` only --
/// a third party's decode of a different build -- and `destiny.Ballpark`
/// exposes no native entry point that puts a ball into them, so our own
/// client cannot be asked. Encoding one on a `[V2]` layout would write a blob
/// whose first wrong field corrupts every record after it.
///
/// **`MISSILE` is no longer one of them** (R-MISSILE §1.1, tracker §83). Its
/// 48-byte tail is `[V]` twice: Carbon's own writer (`Thunkers.cpp`,
/// `case DSTBALL_MISSILE`, tracker §47.2) and the matching arm of our DLL's
/// jump table (`0x180099448`, `docs/re/warp.md`). See [`Mode::Missile`].
///
/// **`WARP` was the fourth and is now `[V]`.** `docs/lanes/round2-spec.md` §8
/// named the way out and its precondition -- once the push path works, warp a
/// ball and have the client's own writer tell us -- and
/// `tools/client-ci/scenarios/warp_blob.py` is that, run. See
/// `docs/port-tracker.md` §38. The same method would settle the other three;
/// what we lack for them is any way to get a ball into the mode at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// `DSTBALL_GOTO`, 24-byte tail. `69-call-GotoPoint.bin`.
    Goto([f64; 3]),
    /// `DSTBALL_FOLLOW`, 12-byte tail. `67-call-FollowBall.bin`, whose range
    /// reads back as `00 00 fa 43` = 500.0, exactly as called.
    Follow { ball_id: i64, range: f32 },
    /// `DSTBALL_STOP`, no tail. `66-call-Stop.bin` is 5 + 100 + 100.
    Stop,
    /// `DSTBALL_ORBIT`, 12-byte tail. `68-call-Orbit.bin`, range 1000.0.
    Orbit { ball_id: i64, range: f32 },
    /// `DSTBALL_TROLL`, 4-byte tail. `71-call-SetBallTroll.bin`, whose stamp
    /// reads back as `currentTime + 1`.
    Troll { effect_stamp: u32 },
    /// `DSTBALL_WARP`, **52-byte tail**, measured from the client's own
    /// writer by `warp_blob.py` and reproduced on two runs
    /// (`docs/port-tracker.md` §38).
    ///
    /// **`goto` is where the ship will *stop*, not what it was aimed at** --
    /// once the warp has activated. Our own `WarpTo` action sends the raw
    /// centre with the standoff in a separate field, and while the ball was
    /// aligning the client's writer echoed that centre to the metre; after
    /// activation it had folded the standoff in, writing the centre pulled
    /// back by exactly `stop_distance`. So a `Mode::Warp` for an observer
    /// joining mid-flight has to pick by the same rule the client does, and
    /// [`Mode::warp_goto`] is that rule in one place.
    ///
    /// **`effect_stamp`'s sign is a state and not a magnitude**: negative
    /// while aligning, where `-stamp / 180` is the progress bar the client
    /// draws (§34.7), and the absolute park tick of activation afterwards.
    /// `michelle.IsPreparingWarp` tests `effect_stamp < 0` and nothing else
    /// `[V]`, so the sign is load-bearing in the client and an `i32` here
    /// rather than the `u32` [`Mode::Troll`] carries.
    Warp {
        goto: [f64; 3],
        effect_stamp: i32,
        /// **The total length of the warp**, in metres: the distance the
        /// ball will cover between activation and dropout.
        ///
        /// `[V]`, and it was an open `[I]` until §46. CCP's own serialiser
        /// writes this slot from `Ball::mLastCollision` with the comment
        /// *"Total length of warp"* -- an **aliased** member, which is why
        /// reading the member name gets it wrong. While the warp has not
        /// activated there is no length yet and the client writes
        /// [`WARP_LENGTH_ALIGNING`]; [`Mode::warp_total_distance`] is that
        /// rule in one place, the same shape as [`Mode::warp_goto`].
        total_distance: f64,
        /// Where the ship stops, as a distance from the target's centre.
        /// Our `service::space::warp` computes it as
        /// `min_range.max(radius + WARP_STANDOFF)`; the measured value was
        /// `radius + 1000` for a planet with a 0 m request.
        stop_distance: f64,
        /// Warp speed in AU/s times a thousand -- 4000 for the capsule's
        /// 4.0 AU/s, which is the same constant `warp_flight_seconds` divides
        /// by 1000 to get its `k_accel` (§37.4).
        warp_factor: i64,
    },
    /// `DSTBALL_MISSILE`, code 5, **48-byte tail** `[V]`: Carbon's
    /// `Thunkers.cpp` `case DSTBALL_MISSILE` and our DLL's jump-table arm at
    /// `0x180099448` agree on the order below (R-MISSILE §1.1).
    ///
    /// Every field is what Carbon's `MissileFollow` sets `[V]`, so
    /// [`Mode::missile`] builds the whole tail from the launch geometry.
    ///
    /// **The reader resolves `follow_id` after the batch** (mode 5 is in the
    /// reader's `0x1032` fix-up mask `[V]`), so the target must already be in
    /// the viewer's park or the client logs `Ball %lld not in park`.
    Missile {
        /// Tail +0: the target ball (`mFollowId`).
        follow_id: i64,
        /// Tail +8: `-(missile radius + target radius)` (`mFollowRange`).
        /// Negative so the missile aims at the target's centre, not its
        /// surface.
        follow_range: f32,
        /// Tail +12: the launcher ball, the ship or the NPC (`mOwnerId`).
        owner_id: i64,
        /// Tail +20: the launch stamp. It must equal the stamp the
        /// `LaunchMissile` action goes out on, or the client's 800 ms
        /// straight-flight window lands on the wrong tick (§2.3).
        effect_stamp: i32,
        /// Tail +24: the launch point plus the launch direction times
        /// [`MISSILE_GOTO_REACH`] (`mGoto`).
        goto: [f64; 3],
    },
}

/// The length of [`Mode::Missile`]'s tail: 8 + 4 + 8 + 4 + 3 x 8. **[V]**
pub const MISSILE_TAIL_LEN: usize = 48;

/// How far along the launch direction a missile's `goto` is put: Carbon's
/// `MissileFollow` writes `velocity.Normalize() * 1e16` **[V]**.
pub const MISSILE_GOTO_REACH: f64 = 1.0e16;

/// What goes at tail offset +28 while a warp has not activated yet.
///
/// **This was an open `[I]` and is now `[V]`** (§46). The field is the warp's
/// total length, and a ball that is still aligning has no length yet, so the
/// client writes `-1.0` there. Every blob this project had ever dumped was of
/// an aligning ball -- the sample in
/// `the_warp_tail_is_fifty_two_bytes_in_the_order_the_client_wrote_them`
/// carries `effect_stamp: -2` -- which is exactly why a sentinel looked like
/// a constant for three sections running.
///
/// The lesson, for the next value that looks like one: a field observed in
/// one state is not a field understood. It took CCP's own comment to tell
/// these apart, and the dump had been saying so all along in the *stamp*.
pub const WARP_LENGTH_ALIGNING: f64 = -1.0;

impl Mode {
    /// The mode byte at record offset `0x08`.
    pub fn code(self) -> u8 {
        match self {
            Mode::Goto(_) => 0,
            Mode::Follow { .. } => 1,
            Mode::Stop => 2,
            Mode::Orbit { .. } => 4,
            Mode::Troll { .. } => 8,
            Mode::Warp { .. } => 3,
            Mode::Missile { .. } => 5,
        }
    }

    /// A [`Mode::Missile`] by Carbon's `MissileFollow` rule **[V]**.
    ///
    /// `from` is the launcher's position (where `LaunchMissile` puts the
    /// missile anyway) and `toward` the target's. The range is
    /// `-(missile_radius + target_radius)` and `goto` is
    /// `from + unit(toward - from) * 1e16`. A target exactly at the launch
    /// point has no direction; `goto` is then `from`, which is finite, and
    /// the client's own launch overwrites the velocity on its first tick.
    pub fn missile(
        target: i64,
        owner: i64,
        effect_stamp: i32,
        missile_radius: f64,
        target_radius: f64,
        from: [f64; 3],
        toward: [f64; 3],
    ) -> Mode {
        let dir = crate::space_state::unit([
            toward[0] - from[0],
            toward[1] - from[1],
            toward[2] - from[2],
        ])
        .unwrap_or([0.0; 3]);
        Mode::Missile {
            follow_id: target,
            follow_range: -((missile_radius + target_radius) as f32),
            owner_id: owner,
            effect_stamp,
            goto: [
                from[0] + dir[0] * MISSILE_GOTO_REACH,
                from[1] + dir[1] * MISSILE_GOTO_REACH,
                from[2] + dir[2] * MISSILE_GOTO_REACH,
            ],
        }
    }

    /// The point to put in a [`Mode::Warp`]'s `goto`, by the client's rule.
    ///
    /// The client's writer echoes the commanded centre while the ball is
    /// still aligning and the arrival point once the warp has activated
    /// (§38.1), and the discriminator is the stamp's sign -- not which
    /// command came first. `from` is where the ship is, which is the side of
    /// the target the arrival point sits on.
    pub fn warp_goto(
        from: [f64; 3],
        centre: [f64; 3],
        stop_distance: f64,
        effect_stamp: i32,
    ) -> [f64; 3] {
        if effect_stamp < 0 {
            centre
        } else {
            warp_arrival(from, centre, stop_distance)
        }
    }

    /// What to put in a [`Mode::Warp`]'s `total_distance`, by the client's
    /// rule: the length once the warp is running, and
    /// [`WARP_LENGTH_ALIGNING`] before it is.
    ///
    /// The discriminator is the stamp's sign, exactly as in
    /// [`Mode::warp_goto`] -- the two fields change meaning together,
    /// because both are written from the same `case DSTBALL_WARP` and both
    /// are undefined until activation.
    pub fn warp_total_distance(effect_stamp: i32, length: f64) -> f64 {
        if effect_stamp < 0 {
            WARP_LENGTH_ALIGNING
        } else {
            length
        }
    }
}

/// The commanded centre of a warp whose arrival point is `arrival`, for a
/// ship at `from`: the inverse of [`warp_arrival`], which puts the arrival
/// `stop` back along the line from the target toward the ship. The ship is
/// on that line, so the direction from `from` to `arrival` is the one to
/// step *forward* along. A ship at the arrival has no line, and gets the
/// arrival itself.
pub fn warp_centre(from: [f64; 3], arrival: [f64; 3], stop: f64) -> [f64; 3] {
    if !stop.is_finite() || stop <= 0.0 {
        return arrival;
    }
    match crate::space_state::unit([
        arrival[0] - from[0],
        arrival[1] - from[1],
        arrival[2] - from[2],
    ]) {
        Some(u) => [
            arrival[0] + u[0] * stop,
            arrival[1] + u[1] * stop,
            arrival[2] + u[2] * stop,
        ],
        None => arrival,
    }
}

/// The physics block that `isFree` gates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Physics {
    pub max_velocity: f32,
    pub velocity: [f64; 3],
    /// The dump's row 7c. Named `inertia` in an earlier draft of
    /// `docs/archive/destiny-and-the-ballpark.md` §4.4 and corrected to `Agility` by
    /// the readback, which is the client's own attribute name.
    pub agility: f32,
    pub speed_fraction: f32,
}

/// One ball.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ball {
    pub ball_id: i64,
    pub mode: Mode,
    pub radius: f32,
    pub position: [f64; 3],
    /// The raw flag byte. **Bit 0 is not read from here** -- [`Ball::physics`]
    /// is what decides it, and [`Park::encode`] sets or clears bit 0 to match,
    /// so the two cannot disagree on the wire.
    pub flags: u8,
    pub mass: f64,
    /// The cloak byte (offset 0x33): `0` visible, else the Destiny cloak mode
    /// (`crate::cloak::MODE_NORMAL`). The client cloaks the ego model itself
    /// when it reads a non-zero one (`spaceObject.Display`).
    pub cloak: u8,
    pub harmonic: i64,
    pub corporation_id: i32,
    pub alliance_id: i32,
    /// `Some` iff `isFree`.
    pub physics: Option<Physics>,
    /// The trailing byte. See [`NO_FORMATION`].
    pub formation_id: u8,
}

/// A whole ballpark state.
#[derive(Debug, Clone, PartialEq)]
pub struct Park {
    /// The 4-byte header clock. `ReadFullStateFromStream` **overwrites** the
    /// park's `currentTime` from this, which the reference dump proves by
    /// round trip: `currentTime after read: 287454020 (wrote 0x11223344)`.
    /// **[V]**
    pub current_time: u32,
    pub balls: Vec<Ball>,
}

impl Park {
    /// The blob, as `ReadFullStateFromStream` reads it.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + self.balls.len() * 100);
        out.push(PACKET_FULL_STATE);
        out.extend_from_slice(&self.current_time.to_le_bytes());
        for b in &self.balls {
            out.extend_from_slice(&b.ball_id.to_le_bytes());
            out.push(b.mode.code());
            out.extend_from_slice(&b.radius.to_le_bytes());
            for v in b.position {
                out.extend_from_slice(&v.to_le_bytes());
            }
            // `physics` is authoritative over bit 0; see the field.
            let flags = match b.physics {
                Some(_) => b.flags | FLAG_IS_FREE,
                None => b.flags & !FLAG_IS_FREE,
            };
            // **A ball in WARP is never massive.** The client's own
            // `ActivateWarp` clears `isMassive` and the arrival sets it again;
            // a massive ball in mode WARP is run through the native collision
            // pass every tick, which writes its first-collision time into the
            // slot the WARP tail's "length" lands in (`ball+0x110`, aliased
            // `mLastCollision`). The warp integrator then reads a length of
            // ~0.9 m, finds the flight long over and logs `Ship stuck in
            // extended warp` + `Goto:` per ball per tick (~330 lines in the
            // owner's 2026-10-28 logs, every one a 0x1f ball). `[V]` as a
            // correlation (all 330 balls were 0x1f, none after 0x1b); the
            // clobbering path is read from `_destiny.dll` `0x180035740`
            // (Evolve) and `0x18003f968` (collision), not run.
            let flags = match b.mode {
                Mode::Warp { .. } => flags & !FLAG_IS_MASSIVE,
                _ => flags,
            };
            out.push(flags);
            out.extend_from_slice(&b.mass.to_le_bytes());
            out.push(b.cloak);
            out.extend_from_slice(&b.harmonic.to_le_bytes());
            out.extend_from_slice(&b.corporation_id.to_le_bytes());
            out.extend_from_slice(&b.alliance_id.to_le_bytes());
            if let Some(p) = b.physics {
                out.extend_from_slice(&p.max_velocity.to_le_bytes());
                for v in p.velocity {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                out.extend_from_slice(&p.agility.to_le_bytes());
                out.extend_from_slice(&p.speed_fraction.to_le_bytes());
            }
            // After the gated block, in both cases.
            out.push(b.formation_id);
            match b.mode {
                Mode::Stop => {}
                Mode::Goto(to) => {
                    for v in to {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                }
                Mode::Follow { ball_id, range } | Mode::Orbit { ball_id, range } => {
                    out.extend_from_slice(&ball_id.to_le_bytes());
                    out.extend_from_slice(&range.to_le_bytes());
                }
                Mode::Troll { effect_stamp } => {
                    out.extend_from_slice(&effect_stamp.to_le_bytes());
                }
                Mode::Warp {
                    goto,
                    effect_stamp,
                    total_distance,
                    stop_distance,
                    warp_factor,
                } => {
                    // **A warp that has not activated has no length yet, and
                    // this blob says so.** §46 read `+28` off CCP's own
                    // serialiser -- `mLastCollision`, an aliased member,
                    // carrying "Total length of warp" -- and recorded that
                    // the client writes [`WARP_LENGTH_ALIGNING`] until
                    // activation. The encoder wrote whatever the record held,
                    // which was correct only because nothing constructed an
                    // unactivated warp.
                    //
                    // §59 changed that: `service::space::warp` now stores the
                    // *predicted* activation stamp, so between the command
                    // and that instant the record holds a real length for a
                    // ship that has not started moving. The discriminator is
                    // this blob's own clock, which is the same field the
                    // client adopts as `mCurrentTime` (§56.1) -- so "has it
                    // activated" is a question the blob can answer about
                    // itself, without a second clock to disagree with.
                    //
                    // **The stamp's sign is deliberately left alone**, and
                    // that is the remaining `[A]` of §59.1: the client would
                    // write a *negative* stamp while preparing, whose
                    // magnitude drives the 180-tick progress bar of §34.7. We
                    // do not know that magnitude's exact derivation, and
                    // inventing one would put a plausible wrong number where
                    // an honest wrong number is. The length is the half we
                    // have evidence for, so it is the half that moves.
                    // **Both halves, and the test caught the missing one.**
                    // Written as `effect_stamp <= current_time` alone, a
                    // client-style negative stamp -- `-1`, "preparing warp"
                    // -- compares as less than any clock and was called
                    // activated, so the blob claimed a length for the one
                    // state that is defined to have none. The sign is a
                    // *state* and the magnitude is a *time* (§38.1); a
                    // comparison that treats the sign as part of the
                    // magnitude conflates them. This is the same pair of
                    // conditions `Motion::advanced_at` guards its curve with,
                    // for the same reason.
                    let activated = effect_stamp >= 0
                        && i64::from(effect_stamp) <= i64::from(self.current_time);
                    let length = if activated {
                        total_distance
                    } else {
                        WARP_LENGTH_ALIGNING
                    };
                    // **An observer is told what the client would write for
                    // a ball that is still aligning** (CAPTURE-UNDOCK): a
                    // negative stamp, the commanded centre and no length.
                    // The server stores the *predicted* activation tick (a
                    // positive stamp in the future) and the arrival point,
                    // and the capture shows both reaching an observer in a
                    // `SetState`/`AddBalls2` taken during the align: mode
                    // WARP, stamp ahead of the blob clock, length -1. The
                    // native then evaluates `WarpDistance` for a ball
                    // "activated" at a tick that has not come, with no
                    // length: `v_c = (d + 1) * ... = 0`, `ln(0)`, a NaN
                    // position that is discarded in silence -- the ball
                    // sits at 0 m, 0 m/s, with a huge angular velocity.
                    // `IsPreparingWarp` (`effect_stamp < 0`) is what then
                    // lets the client run its own align and activate.
                    let (effect_stamp, goto) = if !activated && effect_stamp >= 0 {
                        (-1, warp_centre(b.position, goto, stop_distance))
                    } else {
                        (effect_stamp, goto)
                    };
                    for v in goto {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                    out.extend_from_slice(&effect_stamp.to_le_bytes());
                    out.extend_from_slice(&length.to_le_bytes());
                    out.extend_from_slice(&stop_distance.to_le_bytes());
                    out.extend_from_slice(&warp_factor.to_le_bytes());
                }
                Mode::Missile {
                    follow_id,
                    follow_range,
                    owner_id,
                    effect_stamp,
                    goto,
                } => {
                    out.extend_from_slice(&follow_id.to_le_bytes());
                    out.extend_from_slice(&follow_range.to_le_bytes());
                    out.extend_from_slice(&owner_id.to_le_bytes());
                    out.extend_from_slice(&effect_stamp.to_le_bytes());
                    for v in goto {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
        out
    }

    /// Read a blob back, or `None` if it is not one we can read.
    ///
    /// **The ball list is self-delimiting and has no count**: the native
    /// reader stops when its first 8-byte read comes up short
    /// (`_destiny.dll` `0x180098a36`) **[V]**, so this stops on fewer than 8
    /// bytes remaining and treats a short *record* as an error rather than a
    /// terminator -- the difference between "the park ended" and "we cannot
    /// read this", which a decoder that silently stopped would hide.
    ///
    /// A mode byte we have no tail for is an error for the same reason: the
    /// tail length is not recoverable from the bytes, so continuing would
    /// mis-frame every later record. See [`Mode`].
    pub fn decode(bytes: &[u8]) -> Option<Park> {
        let mut r = Reader { b: bytes, at: 0 };
        if r.u8()? != PACKET_FULL_STATE {
            return None;
        }
        let current_time = r.u32()?;
        let mut balls = Vec::new();
        while r.left() >= 8 {
            let ball_id = r.i64()?;
            let mode_code = r.u8()?;
            let radius = r.f32()?;
            let position = [r.f64()?, r.f64()?, r.f64()?];
            let flags = r.u8()?;
            let mass = r.f64()?;
            let cloak = r.u8()?;
            let harmonic = r.i64()?;
            let corporation_id = r.i32()?;
            let alliance_id = r.i32()?;
            let physics = if flags & FLAG_IS_FREE != 0 {
                Some(Physics {
                    max_velocity: r.f32()?,
                    velocity: [r.f64()?, r.f64()?, r.f64()?],
                    agility: r.f32()?,
                    speed_fraction: r.f32()?,
                })
            } else {
                None
            };
            let formation_id = r.u8()?;
            let mode = match mode_code {
                0 => Mode::Goto([r.f64()?, r.f64()?, r.f64()?]),
                1 => Mode::Follow {
                    ball_id: r.i64()?,
                    range: r.f32()?,
                },
                2 => Mode::Stop,
                4 => Mode::Orbit {
                    ball_id: r.i64()?,
                    range: r.f32()?,
                },
                8 => Mode::Troll {
                    effect_stamp: r.u32()?,
                },
                3 => {
                    let goto = [r.f64()?, r.f64()?, r.f64()?];
                    let effect_stamp = r.i32()?;
                    // Kept, now that it has a name. It used to be read and
                    // dropped because a round trip could not have defended a
                    // value nobody could explain; it is the warp's total
                    // length, so a round trip now preserves it.
                    let total_distance = r.f64()?;
                    Mode::Warp {
                        goto,
                        effect_stamp,
                        total_distance,
                        stop_distance: r.f64()?,
                        warp_factor: r.i64()?,
                    }
                }
                5 => Mode::Missile {
                    follow_id: r.i64()?,
                    follow_range: r.f32()?,
                    owner_id: r.i64()?,
                    effect_stamp: r.i32()?,
                    goto: [r.f64()?, r.f64()?, r.f64()?],
                },
                _ => return None,
            };
            balls.push(Ball {
                ball_id,
                mode,
                radius,
                position,
                flags,
                mass,
                cloak,
                harmonic,
                corporation_id,
                alliance_id,
                physics,
                formation_id,
            });
        }
        Some(Park {
            current_time,
            balls,
        })
    }
}

/// A little-endian cursor. `None` on a short read, every time.
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn left(&self) -> usize {
        self.b.len().saturating_sub(self.at)
    }
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.at.checked_add(N)?;
        let slice = self.b.get(self.at..end)?;
        self.at = end;
        slice.try_into().ok()
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take::<1>()?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take()?))
    }
    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take()?))
    }
    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take()?))
    }
    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take()?))
    }
}

/// One `foo.SlimItem`: what the client needs to draw a ball.
///
/// Three attributes would be enough -- `SlimItem.__getattr__` returns `None`
/// for any name not starting with `__`, derives `groupID`, `categoryID` and
/// `radius` from `typeID` through client-side `evetypes`, and returns `[]`/`0`
/// for `modules`, `jumps`, `ballID` and `bounty` **[V]**.
///
/// `char_id` and `corp_id` are the fourth and fifth because
/// `michelle.ProcessBallAdd` adds an owner to `ownersToPrime` **only when
/// `charID is not None`** **[V]**. Without them the bracket draws and the
/// pilot's name behind it never resolves.
///
/// `response_thresholds` is the sixth and seventh, for an NPC only
/// (`docs/lanes/npc.md` §2): `(hostile, friendly)`, sent as the floats
/// `hostile_response_threshold` / `friendly_response_threshold` when `Some`
/// and absent when `None`, so nothing else's slim changes on the wire.
/// Without them `SlimItem.__getattr__` answers `None`, the bracket logs
/// `failed getting standing thresholds from slim_item` on every refresh
/// (`spacecomponents/client/components/entitystandings.py:57-58`) and
/// `consider.py:89`'s hostility test is false **[V]**.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slim {
    pub item_id: i64,
    pub type_id: i64,
    pub owner_id: i64,
    pub char_id: Option<i64>,
    pub corp_id: Option<i64>,
    pub response_thresholds: Option<(f32, f32)>,
}

impl Slim {
    /// `foo.SlimItem` is whitelisted and old-style with no `__setstate__`, so
    /// the state is an attribute dict and `pyutil::instance` is the right
    /// shape. **[V]**
    /// The same five attributes as a **plain dict**, which is what
    /// `AddBalls2` wants: the client does `slimItem.__dict__ = slimItemDict`
    /// rather than unpickling an instance **[V]**.
    ///
    /// Both forms take the batch's [`ShipModules`]: a ship among them also
    /// carries `modules` ([`SlimModule`]).
    pub(super) fn to_dict(self, modules: &[(i64, Vec<SlimModule>)]) -> PyValue {
        self.to_dict_with(modules, &[])
    }

    #[cfg(test)]
    pub(super) fn to_pyvalue(self, modules: &[(i64, Vec<SlimModule>)]) -> PyValue {
        self.to_pyvalue_with(modules, &[])
    }

    /// [`Self::to_dict`] plus the batch's [`SlimExtras`]: the entry whose
    /// item id is this slim's, if any, after everything else
    /// (`crate::slim_extra`).
    pub(super) fn to_dict_with(
        self,
        modules: &[(i64, Vec<SlimModule>)],
        extras: &[SlimExtra],
    ) -> PyValue {
        crate::pyutil::pydict(slim_pairs(self, modules, extras))
    }

    /// The instance form, with the batch's [`SlimExtras`].
    pub(super) fn to_pyvalue_with(
        self,
        modules: &[(i64, Vec<SlimModule>)],
        extras: &[SlimExtra],
    ) -> PyValue {
        instance("foo.SlimItem", slim_pairs(self, modules, extras))
    }

    /// The attributes both forms carry, in one order.
    pub(super) fn attributes(self) -> Vec<(&'static str, PyValue)> {
        let opt = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
        let mut out = vec![
            ("itemID", PyValue::Int(self.item_id)),
            ("typeID", PyValue::Int(self.type_id)),
            ("ownerID", PyValue::Int(self.owner_id)),
            ("charID", opt(self.char_id)),
            ("corpID", opt(self.corp_id)),
        ];
        if let Some((hostile, friendly)) = self.response_thresholds {
            // **Floats**: the type's own values are floats (`[V-data]`
            // `spaceComponentsByType.entityStandings`) and the client compares
            // them with float standings.
            out.push((
                "hostile_response_threshold",
                PyValue::Real(f64::from(hostile)),
            ));
            out.push((
                "friendly_response_threshold",
                PyValue::Real(f64::from(friendly)),
            ));
        }
        out
    }
}

/// One entry of a ship's `slimItem.modules`: **`(itemID, typeID, flagID)`**.
///
/// **This is how another pilot's client knows what is fitted to a ship it
/// sees.** `TurretSet.FitTurrets` takes two branches (`turretSet.py:428`):
/// for the viewer's own active ship it reads client dogma (`:430-447`); for
/// every other ship it reads `michelle.GetBallpark().GetInvItem(shipID)` --
/// the slim -- and does `for itemID, typeID, flagID in slimItem.modules`
/// (`:453`, `UNPACK_SEQUENCE 3`), slotting each by
/// `GetSlotFromModuleFlagID(flagID)` and fitting it **online, unconditionally**
/// (`:455`, `True`) **[V]**. The same 3-tuple is unpacked a second time by
/// `michelle.GetTypeIDsAndFlagIDsForModules` (`michelle.py:1997`) **[V]**.
///
/// `FitTurret` itself drops anything that is not a fittable turret
/// (`_isFittableTurret`, `turretSet.py:365`) **[V]**, so the list is every
/// fitted module, not only the guns. The set it builds is keyed by `itemID`
/// (`turretsFitted[moduleID] = ts`, `:485`) and lands in `ship.modules`
/// (`ship.py:548`); `StandardWeapon.PopulateTurrets` then looks the gun up as
/// `shipBall.modules.get(self.moduleID)` (`effects/turrets.py:96`) **[V]** --
/// so the `itemID` here must be the id `crate::modules::SpecialFx` sends as
/// `moduleID`, which it is: both are the fitted item's id.
///
/// No charges: a loaded charge has no item id to the client (a
/// `(shipID, flagID, typeID)` sublocation, `crate::item::is_loaded_charge`)
/// and the unpack has no slot for one. An absent `modules` reads as `[]`
/// (`slimItem.py:36-38`) **[V]**, so a ship with nothing fitted sends none.
///
/// A type beside [`Slim`] rather than a field on it, for [`GateSlim`]'s
/// reason: `Slim` is `Copy` and built by struct literal in modules this lane
/// does not own. A ship's list rides beside its slim, keyed by ship id
/// ([`ShipModules`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SlimModule {
    pub item_id: i64,
    pub type_id: i64,
    pub flag_id: i64,
}

impl SlimModule {
    pub(super) fn to_pyvalue(self) -> PyValue {
        PyValue::Tuple(vec![
            PyValue::Int(self.item_id),
            PyValue::Int(self.type_id),
            PyValue::Int(self.flag_id),
        ])
    }
}

/// Every ship's `modules`, by ship item id. A slim whose id is not here, or
/// whose list is empty, sends no `modules` at all.
pub type ShipModules = Vec<(i64, Vec<SlimModule>)>;

pub use crate::slim_extra::{SlimExtra, SlimExtras};

/// A [`Slim`]'s attributes plus its ship's `modules`, if it has any, plus
/// its entry in `extras`, if it has one -- the same lookup `modules` uses.
fn slim_pairs(
    slim: Slim,
    modules: &[(i64, Vec<SlimModule>)],
    extras: &[SlimExtra],
) -> Vec<(&'static str, PyValue)> {
    let mut out = slim.attributes();
    if let Some((_, list)) = modules
        .iter()
        .find(|(id, list)| *id == slim.item_id && !list.is_empty())
    {
        out.push((
            "modules",
            PyValue::List(list.iter().map(|m| m.to_pyvalue()).collect()),
        ));
    }
    // Every source's entry for the slim (Y-WH): a site object that is also a
    // wormhole carries `dunObjectID` from one source and the six hole keys
    // from another; taking only the first dropped the hole's.
    for (_, pairs) in extras.iter().filter(|(id, _)| *id == slim.item_id) {
        out.extend(pairs.iter().cloned());
    }
    out
}

/// A stargate's slim item: a planet's five attributes and **`jumps`**.
///
/// **Without `jumps` a gate is a client traceback.** `SlimItem.__getattr__`
/// answers `[]` for a missing `jumps` **[V]**, and
/// `movementFunctions.DockOrJumpOrActivateGate:410` does `slimItem.jumps[0]`
/// with no guard -- `IndexError` on a double-click. Two consumers read the
/// row, one by name (`movementFunctions.py:413`, `autopilot.py:449`:
/// `.toCelestialID`, `.locationID`) and one by position,
/// `destGate, destSystem = gateItem.jumps[0]` (`subwaySvc.py:82`,
/// `UNPACK_SEQUENCE 2`) **[V]** -- so the row is a `blue.DBRow` with exactly
/// those two columns, in that order, which serves both (`docs/lanes/
/// stargates.md` §3.5). That a `blue.DBRow` unpacks as a sequence is `[I]`:
/// it is how the client's own rowsets are iterated everywhere; falsified by a
/// `TypeError` at `subwaySvc.py:82` on the first jump.
///
/// A type of its own rather than a field on [`Slim`], for the same reason
/// [`WreckSlim`] is: `Slim` is built by struct literal in modules this lane
/// does not own, and a gate is the only thing with a `jumps` row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateSlim {
    pub item_id: i64,
    pub type_id: i64,
    /// `toCelestialID`: the partner gate.
    pub to_gate: i64,
    /// `locationID`: the partner gate's solar system.
    pub to_system: i64,
}

impl GateSlim {
    /// The attribute pairs, shared by both forms. Owner `1` and no pilot or
    /// corporation, as for a planet: the bracket takes the gate's own name
    /// ("Stargate (Akpivem)") from the client's static data.
    pub(super) fn pairs(self) -> Vec<(&'static str, PyValue)> {
        vec![
            ("itemID", PyValue::Int(self.item_id)),
            ("typeID", PyValue::Int(self.type_id)),
            ("ownerID", PyValue::Int(1)),
            ("charID", PyValue::None),
            ("corpID", PyValue::None),
            (
                "jumps",
                PyValue::List(vec![jumps_row(self.to_gate, self.to_system)]),
            ),
            // `clientControllerRegistry.StargateSetup` reads these four when
            // the slim has the attribute at all, and a missing one reads as
            // `None` (so `hasattr` is true) and `None[0]` raises: the client
            // logged "Error adding SpaceObject ... Stargate" and the gate
            // was never added to the scene [V: fci_bursts, 2026-10-28].
            // `(stage, maximum)`; `float(x[0] or 0.0) / float(x[1] or 1.0)`,
            // so `(0, 0)` is "no corruption, no suppression".
            (
                "destinationCorruptionStageAndMaximum",
                PyValue::Tuple(vec![PyValue::Int(0), PyValue::Int(0)]),
            ),
            (
                "localCorruptionStageAndMaximum",
                PyValue::Tuple(vec![PyValue::Int(0), PyValue::Int(0)]),
            ),
            (
                "destinationSuppressionStageAndMaximum",
                PyValue::Tuple(vec![PyValue::Int(0), PyValue::Int(0)]),
            ),
            (
                "localSuppressionStageAndMaximum",
                PyValue::Tuple(vec![PyValue::Int(0), PyValue::Int(0)]),
            ),
        ]
    }

    /// A `foo.SlimItem` instance, as `SetState`'s `slims` wants one.
    pub(super) fn to_pyvalue(self) -> PyValue {
        instance("foo.SlimItem", self.pairs())
    }
}

/// One `jumps` row: `(toCelestialID, locationID)`, both `I8`.
pub fn jumps_row(to_gate: i64, to_system: i64) -> PyValue {
    use eve_marshal::{Column, DbRow, DbRowDescriptor, DbType, DbValue};
    let columns = ["toCelestialID", "locationID"]
        .into_iter()
        .map(|name| Column {
            name: name.as_bytes().to_vec(),
            db_type: DbType::I8,
        })
        .collect();
    let row = DbRow::new(
        DbRowDescriptor::new(columns).expect("two I8 columns are a valid layout"),
        vec![DbValue::Int(to_gate), DbValue::Int(to_system)],
    )
    .expect("two ints match two I8 columns");
    PyValue::DbRow(Box::new(row))
}

#[cfg(test)]
mod missile_tests {
    use super::*;

    // Invented ids (R-MISSILE §3): never a client value.
    const MISSILE: i64 = 9_500_000_000_001;
    const SHIP: i64 = 100_045_001;
    const TARGET: i64 = 100_045_002;

    fn missile_ball() -> Ball {
        Ball {
            ball_id: MISSILE,
            mode: Mode::Missile {
                follow_id: TARGET,
                follow_range: -350.5,
                owner_id: SHIP,
                effect_stamp: 7_001,
                goto: [1.5e16, -2.5e15, 3.25],
            },
            radius: 12.5,
            position: [10.0, 20.0, 30.0],
            flags: 0x05,
            mass: 77.0,
            cloak: 0,
            harmonic: -1,
            corporation_id: 1_000_123,
            alliance_id: 0,
            physics: Some(Physics {
                max_velocity: 4_321.0,
                velocity: [0.0; 3],
                agility: 0.25,
                speed_fraction: 1.0,
            }),
            formation_id: NO_FORMATION,
        }
    }

    fn other_ball() -> Ball {
        Ball {
            ball_id: TARGET,
            mode: Mode::Orbit {
                ball_id: SHIP,
                range: 2_500.0,
            },
            radius: 55.0,
            position: [-1.0, -2.0, -3.0],
            flags: FLAGS_FREE_BALL,
            mass: 1.25e6,
            cloak: 0,
            harmonic: -1,
            corporation_id: 1_000_456,
            alliance_id: 99_000_001,
            physics: Some(Physics {
                max_velocity: 300.0,
                velocity: [1.0, 2.0, 3.0],
                agility: 3.5,
                speed_fraction: 0.5,
            }),
            formation_id: NO_FORMATION,
        }
    }

    /// The tail, byte for byte against R-MISSILE §1.1's layout: 48 bytes,
    /// fields at +0, +8, +12, +20 and +24, little-endian, the mode byte 5.
    #[test]
    fn the_missile_tail_is_forty_eight_bytes_at_the_spec_offsets() {
        let park = Park {
            current_time: 7_001,
            balls: vec![missile_ball()],
        };
        let blob = park.encode();
        // 5-byte header, the 100-byte free record, then the tail.
        let tail_at = 5 + 100;
        assert_eq!(blob.len(), tail_at + MISSILE_TAIL_LEN);
        assert_eq!(MISSILE_TAIL_LEN, 48);
        assert_eq!(blob[5 + 8], 5, "the mode byte is DSTBALL_MISSILE");
        let tail = &blob[tail_at..];
        assert_eq!(tail[0..8], TARGET.to_le_bytes());
        assert_eq!(tail[8..12], (-350.5f32).to_le_bytes());
        assert_eq!(tail[12..20], SHIP.to_le_bytes());
        assert_eq!(tail[20..24], 7_001i32.to_le_bytes());
        assert_eq!(tail[24..32], 1.5e16f64.to_le_bytes());
        assert_eq!(tail[32..40], (-2.5e15f64).to_le_bytes());
        assert_eq!(tail[40..48], 3.25f64.to_le_bytes());
    }

    #[test]
    fn a_missile_ball_round_trips() {
        let park = Park {
            current_time: 7_001,
            balls: vec![missile_ball()],
        };
        assert_eq!(Park::decode(&park.encode()), Some(park));
    }

    /// The corrupt-everything-after case: a wrong tail length would mis-frame
    /// the next record, so a ball written after the missile must read back
    /// exactly.
    #[test]
    fn the_ball_after_a_missile_reads_back_intact() {
        let park = Park {
            current_time: 7_002,
            balls: vec![missile_ball(), other_ball()],
        };
        let blob = park.encode();
        assert_eq!(blob.len(), 5 + 100 + 48 + 100 + 12);
        let back = Park::decode(&blob).expect("a readable park");
        assert_eq!(back.balls.len(), 2);
        assert_eq!(back.balls[1], other_ball());
        assert_eq!(back, park);
    }

    /// `Mode::missile` is `MissileFollow`'s rule: the range is minus both
    /// radii and `goto` lies along the launch line at `MISSILE_GOTO_REACH`.
    #[test]
    fn the_missile_helper_aims_at_the_centre_along_the_launch_line() {
        let from = [100.0, 200.0, 300.0];
        let toward = [100.0, 200.0, 1_300.0];
        let Mode::Missile {
            follow_id,
            follow_range,
            owner_id,
            effect_stamp,
            goto,
        } = Mode::missile(TARGET, SHIP, 42, 12.5, 40.0, from, toward)
        else {
            panic!("a missile mode")
        };
        assert_eq!((follow_id, owner_id, effect_stamp), (TARGET, SHIP, 42));
        assert_eq!(follow_range, -52.5);
        assert_eq!(goto, [100.0, 200.0, 300.0 + MISSILE_GOTO_REACH]);
        // No direction at all still gives a finite, writable tail.
        let Mode::Missile { goto, .. } = Mode::missile(TARGET, SHIP, 42, 1.0, 1.0, from, from)
        else {
            panic!("a missile mode")
        };
        assert_eq!(goto, from);
    }
}
