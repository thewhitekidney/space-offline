//! The in-game store and the vault.
//!
//! Split out of `service.rs`, which had grown to 7374 lines in one file --
//! and that, rather than anything about the work itself, was what capped
//! parallel implementation at one agent. A lane working on this subsystem
//! now opens one file and cannot collide with a lane working on another.
//!
//! The arms are unchanged by the move: `self` became the `reg` argument and
//! nothing else. Helpers deliberately stayed in the parent for now, because
//! several are shared across domains and moving those is where a split of
//! this kind goes wrong.

use super::*;

/// The services dispatched here. `Registry::dispatch` consults this to route.
///
/// `FastCheckoutService` is the one name in the whole server that is not
/// lowerCamel, and that is not a typo: `fastCheckoutClientService.py(29)`
/// does `self.remote_service = sm.RemoteSvc('FastCheckoutService')`, so that
/// capital F is what arrives on the wire. A lowercased copy would be a
/// service we advertise and are never asked for, while the real name stayed
/// unhandled.
pub(crate) const SERVICES: &[&str] = &["FastCheckoutService", "storeManager", "vaultManager"];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    // NES-BACKEND: a catalogue, when the bundle has one, answers the three
    // readers; with none the placeholders below stay (a falsy list would
    // latch the store unavailable for the session).
    if req.service == "storeManager"
        && let Some(v) = crate::nes::store_read(reg, req.method, req.actor)
    {
        return Ok(v);
    }
    match req.service {
        // The PLEX fast-checkout service -- **this is the "bugged
        // inventory" the player reported.**
        //
        // We had no `FastCheckoutService` at all, so `Dispatcher::unhandled`
        // answered `None` (`docs/guides/debugging-the-client.md` §1), and
        // `fastcheckout/client/fastCheckoutClientService.py(75)` does
        //
        //     self._is_china_funnel_enabled, \
        //         self._fake_buy_plex_offer_url, \
        //         self._use_shell_execute_to_buy_plex_offer = \
        //             self.remote_service.GetTestingConfiguration()
        //
        // (`UNPACK_SEQUENCE 3` at offset 12). That is row three of the §4
        // "empty is not one answer" table: a 3-way unpack *before* anything
        // tests for None, so `None` is `TypeError: 'NoneType' object is not
        // iterable`.
        //
        // Where it lands is why the symptom looked like an inventory bug.
        // `aurumStoreContainer._AddPlex` builds the entire PLEX header and
        // then ends on a cosmetic one-liner, verified from the bytecode
        // rather than the decompiler:
        //
        //     363: LOAD_FAST self; LOAD_ATTR fastCheckoutService
        //          LOAD_ATTR is_china_funnel; CALL_FUNCTION
        //          POP_JUMP_IF_FALSE (to 341)
        //     364: LOAD_CONST (2); STORE_ATTR (top)   # plexContainer.top = 2
        //     >>341 LOAD_CONST (None); RETURN_VALUE
        //
        // so the throw is on the **last statement of the method**, and it
        // still aborts `_AddPlex` -> `CreateHeaderLayout` ->
        // `ApplyAttributes` -> `AurumStoreView.SetupStoreData`, leaving the
        // store/vault view half-constructed. From the inventory's PLEX vault
        // that is one click away in two places, both in
        // `plexVault.PlexVaultActions`: `OpenNewEdenStore` ->
        // `uicore.cmd.ToggleAurumStore` (the path above), and `BuyPlex` ->
        // `CmdBuyPlex` -> `buy_plex` -> `is_china_funnel` (the same unpack,
        // one frame earlier).
        //
        // It is also an **LP-variant live lock** (§4). All four readers of
        // the config guard on `is None` and the unpack raises *before* any of
        // the three assignments run, so the cache stays `None` and every
        // later read re-issues the call at UI refresh rate:
        //
        //     def is_china_funnel(self):                      # line 86
        //         if self._is_china_funnel_enabled is None:   # line 87
        //             self._update_config()                   # line 88
        //         return self._is_china_funnel_enabled        # line 89
        //
        // and the same shape at lines 92, 97 and 51. **So `None` for any
        // single element of the tuple re-opens the loop**, which is the
        // constraint that rules out the usual "answer the honest nothing".
        "FastCheckoutService" => match req.method {
            // The config triple. Arity is load-bearing: `UNPACK_SEQUENCE 3`
            // raises the very same `TypeError: too many/not enough values`
            // for a 2-tuple or a 4-tuple that `None` raised, so the fix
            // would have changed the message and nothing else. The test
            // below pins it.
            "GetTestingConfiguration" => Ok(PyValue::Tuple(vec![
                // [0] `_is_china_funnel_enabled` = **True**, and the reason
                // is the jail, not China.
                //
                // This is CCP's flag for the storefront that cannot take
                // card details in the client. `buy_plex`:
                //
                //     151:  if self.is_china_funnel():
                //     152:      sm.GetService('vgsService').OpenStore(
                //                    categoryTag=CATEGORYTAG_PLEX)
                //     153:      return
                //     155:  is_fast_checkout_enabled = \
                //               self.is_fast_checkout_enabled()
                //     158:  if is_fast_checkout_enabled:
                //     159:      self.buy_plex_via_fast_checkout(log_context)
                //     161:  else:
                //               self.buy_plex_online(log_context=log_context)
                //
                // and `buy_plex_online` ends in `blue.os.ShellExecute(...)`
                // (line 213) or `webbrowser.open_new(...)` (line 215) --
                // i.e. it leaves the client. **`True` is the only value that
                // keeps every purchase path in-client unconditionally**: it
                // returns at line 153 before `is_fast_checkout_enabled` is
                // even consulted, and routes the player into the in-game
                // NES instead. `purchaseDetailsPanel._BuyAurum` (line 377)
                // branches on the same flag and reaches
                // `SelectCategoryByCategoryTag(CATEGORYTAG_PLEX)` rather
                // than `CmdBuyPlex`.
                //
                // So this is the project's "the client never phones home"
                // rule expressed as a feature flag. It is worth spelling out
                // because it is a non-obvious reason for a non-obvious
                // value: a real western shard sends `False` here, and
                // `False` is what would send a player to a browser.
                //
                // The cost is two cosmetic effects, both harmless.
                // `plexContainer.top` becomes 2 instead of `CAPTION_OFFSET`
                // (30) -- that is line 364, the statement the bug was
                // hiding. And `should_use_plex()` (line 102,
                // `is_fast_checkout_enabled() and not is_china_funnel()`)
                // goes False, which only picks `_BuyAurum` over
                // `buyAurumFunc` in the NES detail panel -- and `_BuyAurum`
                // then stays in-client for the same reason.
                PyValue::Bool(true),
                // [1] `_fake_buy_plex_offer_url` = an **empty `str`**.
                //
                // Not `None`, because of the `is None` cache at line 92.
                // Falsy, because its one consumer tests truthiness:
                //
                //     224: def _get_url_to_buy_plex_offer(self, offer_id,
                //                                         origin, reason):
                //     225:     fake_url = self._get_fake_buy_plex_offer_url()
                //     226:     if fake_url:
                //     227:         return fake_url
                //     229:     ...build the real store URL...
                //
                // Empty is the honest answer -- this is a QA override and no
                // live shard sets it -- and it costs nothing here, because a
                // truthy fake URL would *still* be handed to
                // `webbrowser.open_new` / `blue.os.ShellExecute` by
                // `buy_plex_offer_online` (lines 248-251). It changes which
                // URL leaves, not whether one does. What keeps purchases
                // in-client is element [0], not this.
                //
                // Falsy also keeps the client on its authored path through
                // `urlUtil.get_url_with_token_and_parameters`, which calls
                // `sm.RemoteSvc('userSvc').GetUserToken()` -- a gap we would
                // rather see in the log than paper over (§1).
                //
                // A `str` rather than `False` or `0`: the field is a URL, and
                // the `if fake_url:` guard is the only thing standing
                // between it and `webbrowser.open_new(plex_url)`.
                bstr(""),
                // [2] `_use_shell_execute_to_buy_plex_offer` = **False**.
                //
                // Not `None` (the `is None` cache at line 97), and `False`
                // selects `webbrowser.open_new(plex_url)` over
                // `blue.os.ShellExecute(plex_url)` in both
                // `buy_plex_online` (212-215) and `buy_plex_offer_online`
                // (248-251).
                //
                // Element [0] stops `buy_plex` reaching either branch, but
                // three call sites reach `buy_plex_online` *directly* and
                // the funnel flag does not cover them:
                // `eveCommands.py(2223)`,
                // `fastcheckout/client/restrictionDialogue.py(48)`, and the
                // "account management" link label in
                // `fastCheckoutStoreView.construct_layout`. For those,
                // `webbrowser` is the safer of the two:
                //
                // - `webbrowser.open_new` is pure Python
                //   (`webbrowser.pyc` line 448: `WindowsDefault.open` ->
                //   `os.startfile(url)`), so whatever it raises is caught by
                //   the `except Exception` wrapping it and becomes
                //   `UserError('FailedToOpenBuyPlexUrl')` (line 222) -- an
                //   in-client dialog, plus a `LogException` we can read.
                // - `blue.os.ShellExecute` is **native**, in `blue.dll`. It
                //   hands the URL to Wine's shell handler, and that is the
                //   one plausible way out of a namespace with no default
                //   route: it can reach the host session's browser over the
                //   shared desktop session rather than over the jail's
                //   network, which §0b's DNS log would never see. [I] --
                //   inferred, not observed, and the wrong way to find out.
                PyValue::Bool(false),
            ])),
            // `is_fast_checkout_enabled`:
            //
            //     50: def is_fast_checkout_enabled(self):
            //     51:     if self._is_fast_checkout_enabled is None:
            //     52:         if is_client_launched_through_steam():
            //     54:             self._is_fast_checkout_enabled = False
            //     56:         else:
            //                       self._is_fast_checkout_enabled = \
            //                           self.remote_service \
            //                               .IsFastCheckoutEnabledForUser()
            //     57:     return self._is_fast_checkout_enabled
            //
            // `False` is both honest and safe. Honest: the flag means "this
            // account has a stored payment token", and we have no payment
            // backend at all. Safe: the guard is `is None`, so `False`
            // caches and is asked for exactly once -- `None` here would be
            // the third live lock in this one service.
            //
            // The usual danger of `False` -- `buy_plex` line 161 falling
            // through to `buy_plex_online` -- cannot fire, because element
            // [0] of the config returns at line 153 first. Note the
            // alternative was worse, not better: `True` opens the in-game
            // fast-checkout window, which then needs priced offers and a
            // working `BuyOffer` below.
            "IsFastCheckoutEnabledForUser" => Ok(PyValue::Bool(false)),
            // Fire-and-forget. `clear_fast_checkout_enabled` is
            //
            //     60: self._is_fast_checkout_enabled = None
            //     61: self.remote_service.ClearFastCheckoutCacheForUser()
            //
            // and the return value is discarded, so `None` is the whole
            // answer. It is here anyway rather than left to
            // `Dispatcher::unhandled`: the local cache is reset on line 60
            // *before* the call, so an UNANSWERED line from here would be
            // indistinguishable noise next to the real ones.
            "ClearFastCheckoutCacheForUser" => Ok(PyValue::None),
            // The offer catalogue. **A falsy answer and a missing key are
            // two different disasters, and `{}` is both.**
            //
            //     132: def _prime_offers_cache(self):
            //     133:     if self.offers:
            //     134:         return
            //     136:     self.offers = self.remote_service.GetOffersForUser()
            //     138:     for offer in self.offers['plex']:
            //     139:         if any(x.endswith('1.0') for x in offer['tags']):
            //     140:             self.plex_base_price = offer['price']
            //     141:             self.plex_base_quantity = offer['baseQuantity']
            //
            // Line 133 is `if self.offers:`, not `is None`, so `{}`, `[]`,
            // `()` and `None` are all falsy and all re-issue the call on
            // every read -- live lock number three. Line 138 subscripts by
            // `'plex'`, so a dict without that key is a `KeyError`, and
            // `None` is a `TypeError`. Raising instead of returning is the
            // *worst* option available: the assignment on line 136 never
            // happens, so a raise is the LP bug exactly.
            //
            // `{'plex': []}` satisfies all three at once: a one-key dict is
            // truthy so it caches, the key exists so no `KeyError`, and the
            // empty list means the loop body never runs, so
            // `plex_base_price` and `plex_base_quantity` stay `None` --
            // which is fine, they are only unpacked as a 2-tuple
            // (`get_plex_offers_base_price_and_quantity`, line 145).
            //
            // Downstream, `_get_offers('plex')` returns `[]` and
            // `get_plex_offers` line 107-108 does `if not offers: raise
            // KeyError()` on purpose. That is CCP's own "no offers for you"
            // signal and it is caught: `fastCheckoutStoreView(53)` turns it
            // into the authored "maximum transactions exceeded" popup. We do
            // not fabricate a priced PLEX offer to avoid that popup -- there
            // is no backend that could honour one, and selecting it would
            // walk straight into `BuyOffer` below.
            "GetOffersForUser" => Ok(pydict(vec![("plex", PyValue::List(vec![]))])),
            // **A real purchase, so it refuses -- and refusing is also the
            // only answer that is not a lie.**
            //
            //     264: def buy_plex_offer(self, log_context, offer, password):
            //     267:     ret = self.remote_service.BuyOffer(
            //                      offer=offer, password=password,
            //                      purchaseTraceID=..., journeyID=...)
            //     273: except UserError as e:
            //     274:     self._log_plex_offer_purchase_error(
            //                  log_context, offer.id, offer.quantity, e.msg)
            //     275:     raise
            //
            // We raise `UserError('OfferNotAvailable')` rather than return,
            // for two reasons. The message is the client's own vocabulary,
            // not ours: `_log_plex_offer_purchase_error(287)` builds the
            // literal map `{'AuthFailed': 'wrong_password',
            // 'OfferNotAvailable': 'offer_unavailable'}` at lines 288-290,
            // so the server is *expected* to send exactly this string, and
            // `purchaseView.send_purchase_confirmation(224-226)` catches
            // `UserError` and shows `e.msg` in the panel. It is an authored,
            // in-client failure.
            //
            // Returning anything is worse. `_is_payment_confirmed(261)` does
            // `ret.get('Message', None)`, so `None` is an `AttributeError`
            // and a `{}` reads as "not confirmed", which makes the client log
            // `log_plex_offer_purchase_needs_web_confirmation` and hand the
            // player on towards a web confirmation -- a phone-home we would
            // have invented ourselves.
            //
            // Unreachable in V24.01 as far as the bytecode shows -- nothing
            // outside `fastCheckoutClientService` calls `buy_plex_offer`, and
            // `purchaseView` goes through the protobuf store-payment service
            // instead (§3c, not ours). Served anyway so that it cannot
            // become a hang if a later path reaches it.
            //
            // **The key is `OfferNotAvailableNow`. Do not "fix" it back to
            // `OfferNotAvailable`** -- there is real evidence for that
            // spelling and it is a trap.
            //
            // The evidence is `_log_plex_offer_purchase_error(287-291)`, which
            // holds `{'OfferNotAvailable': 'offer_unavailable', 'AuthFailed':
            // 'wrong_password'}` and `.get(e.msg, 'purchase_failed')`. So the
            // client really does know that string -- but only as a **log
            // mapping**, and the handler that consults it ends in `raise`
            // (`buy_plex_offer:273-275`, `RAISE_VARARGS` at 275). The
            // exception carries on to the dialog renderer, which needs a key
            // that resolves.
            //
            // `OfferNotAvailable` resolves nowhere: no row in
            // `res:/staticdata/dialogs.static` (4,223 keys) and no
            // `UI/Messages` label (2,974). The player would get a **fatal**
            // "Could not find message with key" modal -- strictly worse than
            // the `None` this arm replaced. `OfferNotAvailableNow` exists,
            // renders "Offer no longer available", and costs only the
            // precision of one log line that falls back to
            // `purchase_failed`.
            //
            // The general rule, which cost us this one: **a key the client
            // mentions is not a key the client can render.**
            // `findings/user-errors.tsv` is the list that can be, and every
            // key we emit belongs in it.
            "BuyOffer" => Err(ServiceError::Raise(eve_proto::user_error(
                "OfferNotAvailableNow",
                vec![],
            ))),
            _ => Err(reg.no_method(req)),
        },
        // The in-game store (NES / PLEX vault), and **the one call where
        // an empty answer is the only wrong one.**
        //
        // Every other "nothing here" in this file is a `{}` or a `[]`.
        // Here the client does
        //
        //     if not reg.available:
        //         raise StoreUnavailableError()            # line 43
        //     offers = sm.RemoteSvc('storeManager').get_offers(store_id)
        //     if not offers:
        //         reg.available = False                   # line 46
        //         raise StoreUnavailableError()            # line 47
        //     return [create_offer_from_json(j) for j in offers
        //             if 'products' in j and len(j['products'])]
        //
        // so `None`, `[]`, `()` and `{}` are all equally falsy and all
        // four **latch** `reg.available = False` for the rest of the
        // session. That latch is why the capture held two
        // StoreUnavailableErrors from the same vault: the first came from
        // line 47 and really did reach us, and the second came from line
        // 43 without touching the wire at all. One bug, two tracebacks,
        // and only the first was ours.
        //
        // The answer is a list that is **truthy and then filters itself
        // empty**. The comprehension drops any entry without a non-empty
        // `products`, so a single offer carrying no products is a real
        // catalogue of zero purchasable things rather than a fabricated
        // one. Nothing downstream is fabricated either: `SearchOffers...`
        // returns `[]`, the price functor returns `None`, and
        // `PlexAction.UpdatePrice` takes its authored `_RemovePriceTag()`
        // branch -- the client's own "this store has nothing for you"
        // path.
        //
        // We do not invent an offer with a price. `get_min_offer_price_in_currency`
        // does `min(prices) or 0` over the PLX-filtered pricings, so a
        // fake offer that matched a typeID without a `PLX` entry would be
        // a `ValueError` on `min([])`.
        //
        // There is no feature flag for "no store on this cluster" --
        // `VgsService.Initialize` builds the real client unconditionally,
        // and `StoreUnavailableError` is raised in exactly one file and
        // caught in none.
        "storeManager" => match req.method {
            "buy_offer" => crate::nes::buy_arm(reg, req),
            "get_offers" => Ok(PyValue::List(vec![pydict(vec![(
                "products",
                PyValue::List(vec![]),
            )])])),
            // The two siblings share `reg.available`, so leaving them
            // falsy would re-latch the store that `get_offers` just
            // un-latched. Re-checked against the PLEX-vault run: it is
            // the *same* latch, not a weaker one --
            // `vgsClient.get_categories` is lines 56-62 and
            // `get_products` 65-71, each a verbatim copy of 42-48. The
            // vault reaches all three, via
            // `plexVault.GetPriceForTypeIdFunctor` ->
            // `vgsService.GetStore()` for the multi-training price tag.
            //
            // `get_products` elements need `id`, `name`, `href`, and
            // nothing filters them -- but the list only has to be truthy,
            // and an entry that names nothing is honest here.
            "get_products" => Ok(PyValue::List(vec![pydict(vec![
                ("id", PyValue::Int(0)),
                ("name", PyValue::Unicode(String::new())),
                ("href", PyValue::Unicode(String::new())),
            ])])),
            // Categories are the one place we must emit a real element:
            // every entry goes through `create_category_from_json` with
            // no filter in front of it, so there is no truthy-but-empty
            // trick. One ROOT category -- `parent` must be `None`,
            // because `_process_categories` resolves a parent id against
            // `categories_by_id` and a dangling one is a `KeyError`.
            // With no children, `GetRootCategoryList()` still comes back
            // empty, so the store UI shows nothing and raises nothing.
            "get_categories" => Ok(PyValue::List(vec![pydict(vec![
                ("id", PyValue::Int(0)),
                ("name", PyValue::Unicode(String::new())),
                ("href", PyValue::Unicode(String::new())),
                ("parent", PyValue::None),
            ])])),
            // `buy_offer` is deliberately **not** served, and the default
            // `None` is the right answer rather than an oversight.
            // `vgsClient.buy_offer(78-83)` is `result = ...; if not
            // result: raise PurchaseFailedError()`, so `None` takes the
            // client's own "purchase failed" path -- and it is
            // unreachable anyway, because `get_offers` above filters its
            // one entry away and no offer can be selected. Serving it
            // would mean inventing a completed purchase.
            _ => Err(reg.no_method(req)),
        },
        // `get_plex_account_balance` is `return balance or 0`, so zero is
        // both the true answer and a safe one.
        //
        // Re-verified on the PLEX-vault run: `vgsClient.py(35)` is
        // `sm.RemoteSvc('vaultManager').get_account_balance(PLX_CURRENCY)`
        // and line 36 is `return balance or 0`, so this arm correctly
        // ignores its one argument and `0` cannot be mistaken for a
        // missing answer. It is **not** the balance the vault shows -- that
        // number comes over the protobuf public gateway
        // (`plex.client.api.repository`), which we do not serve on purpose;
        // see `docs/guides/debugging-the-client.md` §3c.
        "vaultManager" => match req.method {
            // The vault's balance in whole PLEX (`nes_vault`).
            "get_account_balance" => Ok(crate::nes::account_balance(reg, req.actor)),
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;
    use crate::pyutil::dict_get;

    /// Tests for this subsystem belong here, not in the parent. That is the
    /// other half of why the split exists: two lanes adding a method each
    /// would otherwise contend for one `mod tests`.
    /// Every service this module advertises must reach an arm in *this*
    /// module's `dispatch`.
    ///
    /// It used to check the name against a hand-maintained list in the
    /// parent, which is now derived from these very lists -- so that check
    /// had become a tautology. This probes the dispatcher instead, with a
    /// method that cannot exist: `NoSuchMethod` means we got to the service,
    /// `NoSuchService` means the name is advertised and falls straight
    /// through, which is a refused call, which is a hung client.
    #[test]
    fn every_service_here_is_routed_to_this_module() {
        let mut reg = Registry::new(SandboxConfig::default());
        for s in SERVICES {
            let req = Request {
                service: s,
                method: "__no_such_method_exists__",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: None,
            };
            match dispatch(&mut reg, &req) {
                Err(ServiceError::NoSuchMethod { .. }) => {}
                other => panic!(
                    "{s} is advertised by this module but its dispatch answers {other:?} \
                     to a method that cannot exist"
                ),
            }
        }
    }

    /// Call one method on one service in this module, with no arguments.
    fn call(service: &str, method: &str) -> Result<PyValue, ServiceError> {
        let mut reg = Registry::new(SandboxConfig::default());
        let req = Request {
            service,
            method,
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: None,
        };
        dispatch(&mut reg, &req)
    }

    /// **The arity is the fix.** `_update_config` is
    ///
    /// ```text
    /// # fastcheckout/client/fastCheckoutClientService.py(75)
    /// self._is_china_funnel_enabled, self._fake_buy_plex_offer_url, \
    ///     self._use_shell_execute_to_buy_plex_offer = \
    ///         self.remote_service.GetTestingConfiguration()
    /// ```
    ///
    /// which compiles to `UNPACK_SEQUENCE 3`. A 2-tuple or a 4-tuple raises
    /// the very same `TypeError` that `None` raised and leaves the same
    /// half-built store behind, so the count is pinned here explicitly
    /// rather than left implicit in the element assertions below.
    #[test]
    fn get_testing_configuration_is_exactly_a_three_tuple() {
        let got = call("FastCheckoutService", "GetTestingConfiguration").expect("must answer");
        let PyValue::Tuple(items) = &got else {
            panic!("UNPACK_SEQUENCE 3 needs a sequence, got {got:?}");
        };
        assert_eq!(
            items.len(),
            3,
            "UNPACK_SEQUENCE 3 wants exactly 3, got {items:?}"
        );
    }

    /// No element may be `None`, because all three readers cache on
    /// `is None` (lines 87, 92, 97) and a `None` element would re-issue the
    /// call on every read -- the live lock in `docs/guides/debugging-the-client.md`
    /// §4, which is half of what the player saw.
    #[test]
    fn no_element_of_the_testing_configuration_is_none() {
        let got = call("FastCheckoutService", "GetTestingConfiguration").expect("must answer");
        let PyValue::Tuple(items) = &got else {
            panic!("not a tuple: {got:?}")
        };
        for (i, item) in items.iter().enumerate() {
            assert!(
                !matches!(item, PyValue::None),
                "element {i} is None, which re-opens the `is None` live lock"
            );
        }
    }

    /// The china-funnel flag must be `True`: that is the branch at
    /// `buy_plex(151-153)` which opens the in-game NES and returns, instead
    /// of falling through to `buy_plex_online` and its
    /// `webbrowser.open_new` / `blue.os.ShellExecute`. This is the project's
    /// no-phone-home constraint, so it is asserted rather than assumed.
    #[test]
    fn china_funnel_is_on_so_every_purchase_path_stays_in_client() {
        let got = call("FastCheckoutService", "GetTestingConfiguration").expect("must answer");
        let PyValue::Tuple(items) = &got else {
            panic!("not a tuple: {got:?}")
        };
        assert_eq!(
            items[0],
            PyValue::Bool(true),
            "False here sends buy_plex to buy_plex_online, which leaves the client"
        );
    }

    /// The other two must be falsy where the client tests truthiness:
    /// `if fake_url:` at line 226, and `if self._should_use_shell_execute():`
    /// at lines 212 and 248. Falsy *and* not `None` is the narrow target the
    /// two guards leave.
    #[test]
    fn the_url_and_shell_execute_elements_are_falsy_but_not_none() {
        let got = call("FastCheckoutService", "GetTestingConfiguration").expect("must answer");
        let PyValue::Tuple(items) = &got else {
            panic!("not a tuple: {got:?}")
        };
        assert_eq!(
            items[1],
            bstr(""),
            "a truthy fake URL is what `_get_url_to_buy_plex_offer` opens in a browser"
        );
        assert_eq!(
            items[2],
            PyValue::Bool(false),
            "True picks native blue.os.ShellExecute over python's webbrowser"
        );
    }

    /// `_prime_offers_cache` needs **truthy, with a `'plex'` key**: line 133
    /// is `if self.offers:` (not `is None`), so a falsy answer re-issues the
    /// call forever, and line 138 subscripts `self.offers['plex']`, so a
    /// dict without that key is a `KeyError`. `{}` fails both at once.
    #[test]
    fn offers_for_user_is_truthy_and_carries_an_iterable_plex_key() {
        let got = call("FastCheckoutService", "GetOffersForUser").expect("must answer");
        let PyValue::Dict(entries) = &got else {
            panic!("`self.offers['plex']` needs a mapping, got {got:?}");
        };
        assert!(
            !entries.is_empty(),
            "an empty dict is falsy, so `if self.offers:` never caches it"
        );
        match dict_get(&got, b"plex") {
            Some(PyValue::List(offers)) => assert!(
                offers.is_empty(),
                "we have no purchasable PLEX offers and must not invent a priced one"
            ),
            other => panic!("`self.offers['plex']` must be iterable, got {other:?}"),
        }
    }

    /// `is_fast_checkout_enabled` caches on `is None` too (line 51), so this
    /// must be a real bool. `False` is the honest one: there is no stored
    /// payment token because there is no payment backend.
    #[test]
    fn fast_checkout_is_disabled_with_a_real_bool() {
        assert_eq!(
            call("FastCheckoutService", "IsFastCheckoutEnabledForUser").expect("must answer"),
            PyValue::Bool(false)
        );
    }

    /// The cache-clear is fire-and-forget (`clear_fast_checkout_enabled`,
    /// lines 60-61 -- the result is discarded), but it must still be
    /// *answered*: an unanswered `CallReq` is a hang, not a no-op (§1).
    #[test]
    fn clearing_the_cache_is_answered_rather_than_refused() {
        assert_eq!(
            call("FastCheckoutService", "ClearFastCheckoutCacheForUser").expect("must answer"),
            PyValue::None
        );
    }

    /// `BuyOffer` is a real purchase and refuses, with a key the client can
    /// actually **render**.
    ///
    /// This assertion used to require `OfferNotAvailable`, which the client
    /// mentions in a log map but cannot resolve to a dialog -- so the test
    /// was true of the log and false of the screen. See the arm's comment.
    #[test]
    fn buy_offer_raises_a_user_error_the_client_can_render() {
        match call("FastCheckoutService", "BuyOffer") {
            Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) => {
                let PyValue::Tuple(parts) = *header else {
                    panic!("the exception header must be (class, args)")
                };
                assert_eq!(
                    parts[0],
                    PyValue::Global(b"eveexceptions.UserError".to_vec()),
                    "only a globalsWhitelist class survives the client's loader"
                );
                let PyValue::Tuple(args) = &parts[1] else {
                    panic!("UserError.__init__ is called with (msg, dict)")
                };
                assert_eq!(
                    args[0],
                    bstr("OfferNotAvailableNow"),
                    "the msg must be a key that resolves in dialogs.static, \
                     not merely one the client mentions"
                );
            }
            other => panic!("a purchase must refuse, not answer: {other:?}"),
        }
    }

    /// `vgsClient.get_offers(45-47)` latches `reg.available = False` for the
    /// rest of the session on any falsy answer, and `get_categories` /
    /// `get_products` share that flag. The vault reaches all three, so all
    /// three are checked rather than just the one the comment is about.
    #[test]
    fn no_store_manager_list_is_falsy() {
        for method in ["get_offers", "get_products", "get_categories"] {
            match call("storeManager", method) {
                Ok(PyValue::List(items)) => assert!(
                    !items.is_empty(),
                    "storeManager.{method} answered an empty list, which latches \
                     reg.available = False for the session"
                ),
                other => panic!("storeManager.{method} must answer a list, got {other:?}"),
            }
        }
    }

    // ---- NES-BACKEND: the arms that reach the catalogue and the vault ------

    fn pilot_world() -> Registry {
        crate::nes::stocked(crate::testkit::registry_with(|cfg| {
            cfg.characters = vec![crate::model::Character {
                character_id: 3_001,
                owner_user_id: Some(31),
                station_id: Some(600_003),
                ..Default::default()
            }];
        }))
    }

    fn ask(reg: &mut Registry, service: &str, method: &str, args: &[PyValue]) -> PyValue {
        let req = Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(3_001),
        };
        match service {
            "userSvc" => crate::service::base::dispatch(reg, &req),
            _ => dispatch(reg, &req),
        }
        .expect("must answer")
    }

    #[test]
    fn the_catalogue_answers_the_three_readers_with_the_clients_shapes() {
        let mut reg = pilot_world();
        let PyValue::List(offers) = ask(&mut reg, "storeManager", "get_offers", &[PyValue::Int(4)])
        else {
            panic!("a list")
        };
        assert!(offers.len() >= 2);
        assert!(crate::pyutil::dict_get(&offers[0], b"offerPricings").is_some());
        let PyValue::List(cats) = ask(
            &mut reg,
            "storeManager",
            "get_categories",
            &[PyValue::Int(4)],
        ) else {
            panic!()
        };
        assert!(crate::pyutil::dict_get(&cats[0], b"parent").is_some());
        let PyValue::List(products) =
            ask(&mut reg, "storeManager", "get_products", &[PyValue::Int(4)])
        else {
            panic!()
        };
        assert!(crate::pyutil::dict_get(&products[0], b"href").is_some());
    }

    #[test]
    fn buy_offer_through_the_service_pays_from_the_vault_and_a_short_vault_is_falsy() {
        let mut reg = pilot_world();
        let offer = PyValue::Int(crate::nes::fixture::OFFER_ITEM);
        let args = [offer.clone(), crate::pyutil::bstr("PLX"), PyValue::Int(1)];
        // Nothing in the vault: a falsy answer, which the client raises as PurchaseFailedError.
        assert_eq!(
            ask(&mut reg, "storeManager", "buy_offer", &args),
            PyValue::Bool(false)
        );
        assert_eq!(
            ask(
                &mut reg,
                "vaultManager",
                "get_account_balance",
                &[crate::pyutil::bstr("PLX")]
            ),
            PyValue::Int(0)
        );
        crate::nes_vault::credit(reg.deltas_mut(), 31, 100, "gm", 1).unwrap();
        let got = ask(&mut reg, "storeManager", "buy_offer", &args);
        assert_eq!(
            dict_get(&got, b"offerID"),
            Some(&PyValue::Int(crate::nes::fixture::OFFER_ITEM))
        );
        assert_eq!(
            ask(
                &mut reg,
                "vaultManager",
                "get_account_balance",
                &[crate::pyutil::bstr("PLX")]
            ),
            PyValue::Int(30)
        );
        // Another currency, or game time, is not for sale.
        let usd = [offer, crate::pyutil::bstr("USD"), PyValue::Int(1)];
        assert_eq!(
            ask(&mut reg, "storeManager", "buy_offer", &usd),
            PyValue::Bool(false)
        );
        // The token waits in the redeem queue, and a claim in the station delivers it.
        let PyValue::List(tokens) = ask(&mut reg, "userSvc", "GetRedeemTokens", &[]) else {
            panic!()
        };
        assert_eq!(tokens.len(), 1);
        let PyValue::Instance { state, .. } = &tokens[0] else {
            panic!()
        };
        let Some(PyValue::Int(id)) = dict_get(state, b"tokenID") else {
            panic!()
        };
        let claim = [
            PyValue::List(vec![PyValue::Tuple(vec![PyValue::Int(*id), PyValue::None])]),
            PyValue::Int(3_001),
        ];
        let out = ask(&mut reg, "userSvc", "ClaimRedeemTokens", &claim);
        let Some(PyValue::List(made)) = dict_get(&out, b"itemsCreated") else {
            panic!()
        };
        assert_eq!(made.len(), 1);
        assert_eq!(
            ask(&mut reg, "userSvc", "GetRedeemTokens", &[]),
            PyValue::List(vec![])
        );
    }
}
