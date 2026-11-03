//! World -> gateway notices (GW-OUT, `docs/lanes/gw-out.md`).
//!
//! `gateway::publish` writes to sockets, so it must never run under the world
//! lock. World code therefore does not call it: it [`queue`]s a notice on the
//! [`GatewayOutbox`] world slot, under the lock it already holds. The turn
//! that held the lock then [`take`]s the queue before it drops the guard
//! (`dispatch`, after `pending_pushes`; the clock's finish closure in
//! `push`), and calls [`flush`] after the guard is gone. Same shape as the
//! pending pushes. [`flush`] publishes to the one hub `server.rs` installed
//! with [`install_hub`]; with none installed (no `--gateway`, tests) the
//! notices are dropped, which is what a pilot with no gateway stream gets.
//!
//! A notice is lost to a stream that is not open (`gateway::publish`), not
//! kept: the client re-reads state when it opens a window, so a notice is a
//! "refresh now", never the only copy of anything.
//!
//! The empty path is free: [`take`] returns `Vec::new()` without allocating
//! when nothing is queued, and [`flush`] returns before touching the hub.

use crate::gateway::{NoticeHub, Target, publish};
use std::sync::OnceLock;

/// One queued notice: the proto type name, its encoded payload, its audience.
pub type Queued = (String, Vec<u8>, Target);

/// What the world has to say to gateway clients, in order.
#[derive(Default)]
pub struct GatewayOutbox {
    q: Vec<Queued>,
}

/// The world slot. Never persisted.
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "gateway_out",
    make: |_| Box::new(GatewayOutbox::default()),
};

/// Queue a notice. Under the world lock; sends nothing.
pub fn queue(reg: &mut crate::service::Registry, ty: &str, payload: Vec<u8>, target: Target) {
    reg.ext_mut::<GatewayOutbox>()
        .q
        .push((ty.to_owned(), payload, target));
}

/// Move the queued notices out, to be [`flush`]ed after the guard drops.
/// Allocation-free when the queue is empty.
pub fn take(reg: &mut crate::service::Registry) -> Vec<Queued> {
    let q = &mut reg.ext_mut::<GatewayOutbox>().q;
    if q.is_empty() {
        return Vec::new();
    }
    std::mem::take(q)
}

static HUB: OnceLock<NoticeHub> = OnceLock::new();

/// Install the hub [`flush`] publishes to: the one the gateway was spawned
/// with. Set once; a second call is ignored (a test world, a second server in
/// one process keeps the first).
pub fn install_hub(hub: &NoticeHub) {
    let _ = HUB.set(hub.clone());
}

/// Publish `batch` to `hub`, in order. Returns how many stream deliveries
/// happened. **No world guard may be held** (see the scan test below).
pub fn publish_all(hub: &NoticeHub, batch: Vec<Queued>) -> usize {
    batch
        .into_iter()
        .map(|(ty, payload, target)| publish(hub, &ty, &payload, target))
        .sum()
}

/// [`publish_all`] to the installed hub. A no-op for an empty batch or no hub.
pub fn flush(batch: Vec<Queued>) {
    if batch.is_empty() {
        return;
    }
    if let Some(hub) = HUB.get() {
        publish_all(hub, batch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> crate::service::Registry {
        crate::service::Registry::new(crate::model::SandboxConfig::default())
    }

    #[test]
    fn an_empty_outbox_takes_nothing_and_allocates_nothing() {
        let mut w = world();
        let v = take(&mut w);
        assert!(v.is_empty());
        assert_eq!(v.capacity(), 0, "no allocation for an empty turn");
        // And the slot keeps its own buffer when emptied, not a fresh one per turn.
        queue(&mut w, "t", vec![1], Target::Character(1));
        assert_eq!(take(&mut w).len(), 1);
        assert_eq!(take(&mut w).capacity(), 0);
        flush(Vec::new());
    }

    #[test]
    fn queued_notices_come_out_in_order() {
        let mut w = world();
        queue(&mut w, "a", vec![1], Target::Character(1));
        queue(&mut w, "b", vec![2], Target::Character(2));
        let v = take(&mut w);
        let names: Vec<_> = v.iter().map(|q| q.0.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert!(take(&mut w).is_empty());
    }

    // ---- structure: publish never runs under a world guard ----------------

    /// The production text of a source file: before its test module.
    fn production(src: &str) -> &str {
        src.split("\nmod tests {").next().unwrap_or(src)
    }

    /// A source scan. `gateway::publish` is reached only through
    /// `gateway_out::flush`/`publish_all`; no module but this one and
    /// gateway.rs names it; and in the two drains `flush` comes after the
    /// world guard is dropped.
    #[test]
    fn publish_is_never_called_while_a_world_guard_is_held() {
        for (name, src) in [
            ("dispatch.rs", include_str!("dispatch.rs")),
            ("push.rs", include_str!("push.rs")),
            ("service.rs", include_str!("service.rs")),
            ("gateway_plans.rs", include_str!("gateway_plans.rs")),
            ("gateway_ext.rs", include_str!("gateway_ext.rs")),
            ("server.rs", include_str!("server.rs")),
        ] {
            let p = production(src);
            assert!(
                !p.contains("gateway::publish(") && !p.contains("gateway::publish_all("),
                "{name} publishes to the gateway directly; queue on the outbox"
            );
        }
        // The turn: take under the guard, flush after `drop(w)`.
        let d = production(include_str!("dispatch.rs"));
        let take = d.find("gateway_out::take(&mut w)").expect("the turn takes");
        let flush = d.find("gateway_out::flush(").expect("the turn flushes");
        let drop_w = d[take..].find("drop(w);").expect("the guard drops") + take;
        assert!(take < drop_w && drop_w < flush, "flush must follow drop(w)");
        assert_eq!(d.matches("gateway_out::flush(").count(), 1);
        // The clock: take inside the finish closure (guard held), flush after
        // `clock_slices_gated` returned (guards dropped).
        let p = production(include_str!("push.rs"));
        let take = p.find("gateway_out::take(w)").expect("the clock takes");
        let flush = p.find("gateway_out::flush(").expect("the clock flushes");
        let call = p[..take].rfind("clock_slices_gated(").expect("the call");
        assert!(call < take && take < flush);
        assert_eq!(p.matches("gateway_out::flush(").count(), 1);
        // And inside this module only `flush`/`publish_all` call `publish`.
        let me = production(include_str!("gateway_out.rs"));
        assert_eq!(me.matches(" publish(hub").count(), 1);
    }
}
