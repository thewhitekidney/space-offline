//! The `blue.marshal.globalsWhitelist` this client build installs.
//!
//! Anything a server marshals as a `0x17` instance (its dotted class name) or
//! as a `0x02` global (a callable in an ObjectEx header) is resolved through
//! this table; a miss raises
//! `HACKER WARNING! object %s.%s is not in whitelist` and the read is
//! aborted. Nothing else in the client gates these codes, so this list is the
//! complete set of class names a server may name.
//!
//! **Confirmed**, extracted from the client's own
//! `carbon/common/lib/whitelist.py` (`out/pyc/carbon/common/lib/whitelist.pyc`,
//! unobfuscated CCP source). `InitWhitelist()` splits the `whitelist` string
//! on whitespace, resolves each name with `__import__` + `getattr`, and keys
//! the dict **by the resolved object** — so the dotted strings below are
//! exactly the spellings a server may put on the wire.
//!
//! The module builds that string from four literals and `+=`s them
//! conditionally:
//!
//! | literal | entries | included when |
//! |---|---|---|
//! | base | 92 (91 unique; `moniker.Moniker` is listed twice) | always |
//! | common | 72 | always |
//! | server | 88 | `boot.role == 'server'` |
//! | client | 6 | `boot.role == 'client'` |
//!
//! This build's `boot.role` is `client`, so the names below are base + common
//! + client: **169 unique entries**.
//!
//! `InitWhitelist` then also adds every `exceptions.*Error` object, but those
//! are reachable by their own `exceptions.…` names, which the base literal
//! already lists.

/// Every dotted name the client's whitelist resolves, in source order.
pub const GLOBALS: [&str; 169] = [
    "blue.DBRowDescriptor",
    "blue.DBRow",
    "eveexceptions.SQLError",
    "eveexceptions.UserError",
    "eveexceptions.CancelSessionChange",
    "eveexceptions.UnmarshalError",
    "eveexceptions.ConnectionError",
    "eveexceptions.ServiceNotFound",
    "stackless_response_router.exceptions.TimeoutException",
    "carbon.common.script.sys.crowset.CRowset",
    "carbon.common.script.sys.crowset.CIndexedRowset",
    "carbon.common.script.sys.crowset.CFilterRowset",
    "exceptions.AttributeError",
    "exceptions.IndexError",
    "exceptions.ValueError",
    "exceptions.KeyError",
    "exceptions.WindowsError",
    "exceptions.ReferenceError",
    "exceptions.RuntimeError",
    "exceptions.IOError",
    "exceptions.GPSException",
    "exceptions.GPSTransportClosed",
    "exceptions.GPSBadAddress",
    "exceptions.GPSAddressOccupied",
    "carbon.common.script.net.GPSExceptions.GPSException",
    "carbon.common.script.net.GPSExceptions.GPSTransportClosed",
    "carbon.common.script.net.GPSExceptions.GPSBadAddress",
    "carbon.common.script.net.GPSExceptions.GPSAddressOccupied",
    "carbon.common.script.net.machoNetExceptions.MachoException",
    "carbon.common.script.net.machoNetExceptions.MachoWrappedException",
    "carbon.common.script.net.machoNetExceptions.ProxyRedirect",
    "carbon.common.script.net.machoNetExceptions.UberMachoException",
    "carbon.common.script.net.machoNetExceptions.UnMachoDestination",
    "carbon.common.script.net.machoNetExceptions.UnMachoChannel",
    "carbon.common.script.net.machoNetExceptions.WrongMachoNode",
    "xmlrpclib.Fault",
    "carbon.common.script.net.machoNetPacket.CallReq",
    "carbon.common.script.net.machoNetPacket.CallRsp",
    "carbon.common.script.net.machoNetPacket.ErrorResponse",
    "carbon.common.script.net.machoNetPacket.IdentificationReq",
    "carbon.common.script.net.machoNetPacket.IdentificationRsp",
    "carbon.common.script.net.machoNetAddress.MachoAddress",
    "carbon.common.script.net.machoNetPacket.Notification",
    "carbon.common.script.net.machoNetPacket.PingReq",
    "carbon.common.script.net.machoNetPacket.PingRsp",
    "carbon.common.script.net.machoNetPacket.SessionChangeNotification",
    "carbon.common.script.net.machoNetPacket.SessionInitialStateNotification",
    "carbon.common.script.net.machoNetPacket.TransportClosed",
    "carbon.common.script.net.objectCaching.CacheOK",
    "carbon.common.script.net.objectCaching.CachedMethodCallResult",
    "carbon.common.script.net.objectCaching.CachedObject",
    "carbon.common.script.net.cachedObject.CachedObject",
    "utillib.KeyVal",
    "carbon.common.script.net.moniker.Moniker",
    "carbon.common.script.util.format.PasswordString",
    "carbon.common.script.sys.row.Row",
    "carbon.common.script.net.moniker.UpdateMoniker",
    "__builtin__.str",
    "__builtin__.unicode",
    "__builtin__.set",
    "collections.OrderedDict",
    "collections.defaultdict",
    "collections.deque",
    "localizationUtil.LocalizationSafeString",
    "eve.common.script.paperDoll.paperDollDefinitions.SculptingRow",
    "eve.common.script.paperDoll.paperDollDefinitions.ModifierRow",
    "eve.common.script.paperDoll.paperDollDefinitions.ColorSelectionRow",
    "eve.common.script.paperDoll.paperDollDefinitions.AppearanceRow",
    "machonethelpers.argchecker.ValidateArgTypeException",
    "fsd.schemas.loaders.objectLoader.ObjectLoader",
    "fsd.schemas.path.FsdDataPathObject",
    "fsd.schemas.binaryLoader.LoaderState",
    "fsd.schemas.loaders.dictLoader.DictLoader",
    "raffles.common.raffle.Raffle",
    "raffles.common.ticket.Ticket",
    "raffles.common.raffle_data.RaffleCreationData",
    "raffles.RafflesError",
    "raffles.RaffleNotFoundError",
    "raffles.TicketUnavailableError",
    "raffles.NotEnoughISKError",
    "raffles.FailureToDeliverItemError",
    "raffles.FetchError",
    "raffles.InventoryError",
    "raffles.CreateError",
    "raffles.PurchaseError",
    "kiring.common.errors.KiringRedeemingCodeException",
    "kiring.common.errors.KiringRedeemingCodeNotValidException",
    "kiring.common.errors.KiringRedeemingCodeUsedException",
    "kiring.common.errors.KiringRedeemingCodeTypeUsedException",
    "contracts.contractStruct.ContractStruct",
    "uuid.UUID",
    "foo.SlimItem",
    "eve.common.script.util.slimItem.SlimItem",
    "eve.common.script.sys.eveCfg.Standings",
    "eve.common.script.sys.rowset.RowDict",
    "eve.common.script.sys.rowset.RowList",
    "eve.common.script.sys.rowset.Rowset",
    "eve.common.script.sys.rowset.FilterRowset",
    "eve.common.script.sys.rowset.IndexRowset",
    "eve.common.script.sys.rowset.IndexedRows",
    "eve.common.script.sys.eveCfg.StackSize",
    "eve.common.script.sys.eveCfg.Singleton",
    "eve.common.script.util.pagedCollection.PagedResultSet",
    "dogma.exceptions.EmbarkOnlineError",
    "evewar.util.War",
    "eve.common.script.universe.locationWrapper.SolarSystemWrapper",
    "industry.job.Location",
    "industry.const.Error",
    "industry.const.Reference",
    "crimewatch.corp_aggression.settings.AggressionSettings",
    "shipskins.LicensedSkin",
    "eve.common.script.dogma.effect.BrainEffect",
    "dogma.attributes.Attribute",
    "dogma.attributes.SkillLevelAttribute",
    "dogma.attributes.ChargedAttribute",
    "dogma.attributes.HeatAttribute",
    "dogma.attributes.StackingNurfedAttribute",
    "dogma.attributes.LiteralAttribute",
    "dogma.attributes.BrainLiteralAttribute",
    "datetime.datetime",
    "datetime.date",
    "projectdiscovery.common.exceptions.NoActiveClassificationTaskError",
    "projectdiscovery.common.exceptions.UnexpectedMmosResultError",
    "projectdiscovery.common.exceptions.TooFewPolygonsMmosResultError",
    "projectdiscovery.common.exceptions.TooFewVerticesMmosResultError",
    "projectdiscovery.common.exceptions.NoConnectionToAPIError",
    "projectdiscovery.common.exceptions.MissingKeyError",
    "marketutil.BestByOrder",
    "marketutil.OrderAllData",
    "seasons.common.challenge.Challenge",
    "seasons.common.exceptions.ChallengeForCharacterNotFoundError",
    "seasons.common.exceptions.NotEnoughChallengeTypesAvailableError",
    "seasons.common.exceptions.ChallengeTypeNotFoundError",
    "explorationscanner.common.cosmicAnomalyInfo.CosmicAnomalyInfo",
    "explorationscanner.common.signatureInfo.SignatureInfo",
    "explorationscanner.common.structureInfo.StructureInfo",
    "explorationscanner.common.staticSiteInfo.StaticSiteInfo",
    "evedungeons.common.dungeonInfo.DungeonInfo",
    "agency.common.contentdata.CustomContentData",
    "fastcheckout.common.errors.FastCheckoutDisabled",
    "characterskills.common.character_skill_entry.CharacterSkillEntry",
    "npcs.npccorporations.CorporationTicker",
    "characterenergy.characterEnergyState.CharacterEnergyState",
    "characterenergy.characterEnergyState.InsufficientEnergyError",
    "solarsysteminterference.solarsystemInterferenceState.SolarsystemInterferenceState",
    "solarsysteminterference.solarsystemInterferenceState.ExcessInterferenceError",
    "structures.structureHangarViewState.StructureHangarViewState",
    "factionwarfare.occupationState.OccupationState",
    "factionwarfare.victoryPointState.VictoryPointState",
    "factionwarfare.advantageState.AdvantageState",
    "shipcosmetics.structs._modules._logo.ShipCosmeticLogo",
    "shipcosmetics.structs._fitting._group.ShipCosmeticFittingGroup",
    "shipcosmetics.structs._fitting._fitting.ShipCosmeticFitting",
    "shipcosmetics.structs._license._logo.ShipLogoLicense",
    "shipcaster.landingPadData.LandingPadData",
    "deathzone.targetTrackingState.TargetTrackingState",
    "pirateinsurgency.campaignClientSnapshot.CampaignClientSnapshot",
    "lpOffers.exceptions.TransfersDisabledError",
    "sovereignty.data_types.SovHubInfo",
    "sovereignty.data_types.SovClaimInfo",
    "dotWeapons.common.dotAppInfo.DotApplicationInfo",
    "evedungeons.common.instance_identifier.DungeonInstanceIdentifier",
    "jumpbridge.common.jump_bridge_authorized_info.JumpBridgeAuthorizedInfo",
    "playerprogression.assetnetworthnotifier.AssetNetWorthProgression",
    "playerprogression.contractprogressionnotifier.ContractProgression",
    "playerprogression.marketprogressionnotifier.MarketProgression",
    "raffles.common.const.BlueprintType",
    "campaigns.client.v1.data_model.campaign.CampaignID",
    "campaigns.client.v2.data_model.campaign.CampaignID",
];

/// Is `name` a dotted global this client will resolve?
///
/// A server should check this before emitting a `0x17` class name or a `0x02`
/// global, because the alternative is the client aborting the whole read.
pub fn is_whitelisted(name: &[u8]) -> bool {
    GLOBALS.iter().any(|g| g.as_bytes() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_the_clients_own_and_has_no_duplicates() {
        assert_eq!(GLOBALS.len(), 169);
        let mut sorted = GLOBALS;
        sorted.sort_unstable();
        for pair in sorted.windows(2) {
            assert_ne!(pair[0], pair[1], "duplicate entry");
        }
        // The first two entries of the base literal, in order.
        assert_eq!(GLOBALS[0], "blue.DBRowDescriptor");
        assert_eq!(GLOBALS[1], "blue.DBRow");
    }

    #[test]
    fn every_name_is_dotted_and_resolvable() {
        for g in GLOBALS {
            // The resolver splits on the LAST dot and imports the prefix; a
            // name with no dot would be looked up in __builtin__.
            assert!(g.contains('.'), "{g} has no module part");
            assert!(!g.ends_with('.'), "{g} has an empty attribute");
            assert!(!g.starts_with('.'), "{g} has an empty module");
        }
    }

    #[test]
    fn the_names_this_crate_emits_are_all_whitelisted() {
        for n in [
            crate::keyval::KEY_VAL_CLASS,
            crate::row::ROW_CLASS,
            crate::crowset::CROWSET_CLASS,
            crate::crowset::CINDEXED_ROWSET_CLASS,
            crate::crowset::CFILTER_ROWSET_CLASS,
            eve_marshal::dbrow::DESCRIPTOR_GLOBAL,
            eve_marshal::dbrow::ROW_GLOBAL,
        ] {
            assert!(
                is_whitelisted(n),
                "{} is not whitelisted",
                String::from_utf8_lossy(n)
            );
        }
    }

    #[test]
    fn a_plausible_but_absent_name_is_refused() {
        // MachoPacket itself is not whitelisted -- only its subclasses.
        assert!(!is_whitelisted(
            b"carbon.common.script.net.machoNetPacket.MachoPacket"
        ));
        assert!(is_whitelisted(
            b"carbon.common.script.net.machoNetPacket.CallRsp"
        ));
        // dbutil.CRowset is the __guid__, not the whitelisted dotted path.
        assert!(!is_whitelisted(b"dbutil.CRowset"));
    }
}
