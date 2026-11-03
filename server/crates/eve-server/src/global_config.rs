//! `machoNet.InitClientGlobalConfig` — the one call this server makes.
//!
//! **Everything else we send is a reply or a notification.** This is a
//! `CALL_REQ` addressed to a service *on the client*, which is a direction
//! nothing here had ever used. `machoNet.__clientallowedcommands__` permits
//! `MACHONETMSG_TYPE_CALL_REQ` and `ServiceCallGPCS.CallUp` gates only on a
//! `__restrictedcalls__` deny list that machoNet does not have, so the method
//! is callable — `docs/port-tracker.md` §41.4 and §41.5 have the derivation
//! and the client-side route.
//!
//! **Why it exists.** On a client `machoNet.GetGlobalConfig` is
//! `elif macho.mode == 'client' and self.globalConfig is None: return {}`
//! **[V]**, so until this call arrives *every* `globalConfig` flag in the
//! client sits at its compiled-in default. That is why the client asks
//! `essMgr` for ESS data we do not have — `ess_disabled` defaults to `False`.
//! Setting it is CCP's own off switch, which is better than answering `None`
//! and hoping: the client then takes its own documented path
//! (`dynamicResourceSvc.GetESSDataForCurrentSystem` returns `None` by itself)
//! and never asks.
//!
//! **This route is CLOSED, and the client is what closed it.** The module is
//! kept because the frame and the flag names are correct and hard-won, and
//! because the next person to want a `globalConfig` value should find the
//! dead end documented rather than rediscover it.
//!
//! Sent for real on 2026-10-24, the client answered:
//!
//! ```text
//! exceptions.RuntimeError: In machoNet, method InitClientGlobalConfig
//!                          is not exported
//!   ... ObjectCallGPCS.py(214) CallUp
//! ```
//!
//! §41.5 concluded the call was permitted because `ServiceCallGPCS.CallUp`
//! gates on a `__restrictedcalls__` deny list. **That was the wrong layer.**
//! The traceback names `ObjectCallGPCS`, which enforces `__exportedcalls__` --
//! and `InitClientGlobalConfig` is not in machoNet's. One client run falsified
//! three sections of careful reading, which is exactly what §41.9 said the run
//! was for.
//!
//! The ESS call this was meant to silence is now answered with `None` in
//! `service::space`, which is the value the client's own code returns on that
//! path. §41.10.

use crate::pyutil::{bstr, pypairs};
use eve_marshal::{MachoNetMsg, PyValue};
use eve_proto::{CallPayload, MachoAddress, MachoPacket, ProtoError};

/// The call id we use, deliberately far from the client's own.
///
/// A client's ids start at 1 and climb one per call, so a low number would
/// eventually collide and make the client match our call against one of its
/// own.
pub const CALL_ID: i64 = 0x7000_0001;

/// The flags we turn off, with the key names read from
/// `globalConfig/const.py` **[V]**.
///
/// The values go through `GetGlobalConfigBooleanValue`'s `bool(int(value))`,
/// so each is a string an `int()` accepts.
pub const FLAGS: [(&str, &str); 2] = [
    // `:89` — `ESS_FEATURE_FLAG_DISABLED_CONFIG`, default `False`.
    ("ess_disabled", "1"),
    // `:91` — `ESS_RESERVE_BANK_DISABLED_CONFIG`, default `False`.
    ("ess_reserve_disabled", "1"),
];

/// The config dict, in **wire order**.
///
/// A marshal dict pair is `(value, key)` and [`pypairs`] is the one place that
/// inversion is written down. §39 is what happened when it was spelled out a
/// second time and got backwards.
pub fn config_dict() -> PyValue {
    PyValue::Dict(pypairs(FLAGS.iter().map(|(k, v)| (*k, bstr(v))).collect()))
}

/// The whole frame, ready to write.
pub fn packet(node_id: i64, client_id: i64) -> Result<Vec<u8>, ProtoError> {
    let payload = CallPayload::Service {
        method: "InitClientGlobalConfig".to_owned(),
        args: vec![config_dict()],
        kwargs: Vec::new(),
    };
    MachoPacket::new(
        MachoNetMsg::CallReq,
        MachoAddress::node(node_id),
        MachoAddress::Client {
            client_id,
            call_id: Some(CALL_ID),
            service: Some("machoNet".to_owned()),
        },
        vec![payload.to_payload()],
    )
    .to_payload()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyutil::dict_get;

    /// **The frame the client has to be able to read**, checked field by
    /// field against what `ServiceCallGPCS.CallUp` does with it.
    ///
    /// `CallUp` reads the payload positionally: `[0]` truthy selects the
    /// service-call branch, `[1]` is the method, `[2]` the args, `[3]` the
    /// kwargs. `CallPayload::Service` already encodes exactly that, so this
    /// asserts the parts that are ours to get wrong — the direction, the
    /// destination service, the call id, and the dict's slot order.
    #[test]
    fn the_global_config_call_is_addressed_to_the_clients_machonet() {
        let bytes = packet(42, 7).expect("a frame");
        let value = crate::loads(&bytes).expect("it parses back");
        let packet = MachoPacket::from_value(&value).expect("a macho packet");

        assert_eq!(
            packet.command,
            MachoNetMsg::CallReq,
            "a plain method needs a CALL_REQ; a notification would be \
             delivered as a ScatterEvent and machoNet has no handler for one"
        );
        assert_eq!(
            packet.destination.service(),
            Some("machoNet"),
            "CallUp resolves destination.service through ConnectToService, so \
             the name is how the call finds the service at all"
        );
        assert_eq!(packet.destination.call_id(), Some(CALL_ID));
        // No `node_id()` accessor exists, so the address type is the
        // assertable part: it must come from a Node and not from a Client.
        assert_eq!(
            packet.source.address_type(),
            eve_marshal::AddressType::Node,
            "it comes from our node; a client source would make the client              think another client called it"
        );
        assert_eq!(
            packet.destination.address_type(),
            eve_marshal::AddressType::Client,
            "and it is addressed at a client, which is the direction that is              new here"
        );
    }

    /// **The dict is value-first, which is the §39 lesson as a test.**
    ///
    /// `GetGlobalConfigValue` is `machoNet.GetGlobalConfig().get(configName,
    /// default)`, so the client reads this as a Python dict keyed by the flag
    /// name. A pair built `(key, value)` would make the client's `.get` miss
    /// and every flag fall back to its default — the same silent nothing
    /// `CmdAlignTo` did for months.
    #[test]
    fn the_config_dict_is_keyed_the_way_the_client_reads_it() {
        let d = config_dict();
        for (key, want) in FLAGS {
            assert_eq!(
                dict_get(&d, key.as_bytes()),
                Some(&bstr(want)),
                "{key} has to be readable as a key; a (key, value) pair would \
                 put the flag name in the value slot and the client's .get \
                 would miss it"
            );
        }
        // And the asymmetry, so a flipped builder cannot satisfy this.
        assert_eq!(
            dict_get(&d, b"1"),
            None,
            "the value must not be readable as a key"
        );
    }
}
