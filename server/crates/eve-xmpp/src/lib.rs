//! The XMPP wire format the EVE client's chat speaks, and a client for it.
//!
//! No game knowledge lives here: this is bytes and stanzas. The chat hub
//! (rooms, membership, who hears what) is `eve-server`'s `chat` module, and
//! the protocol it implements is `docs/lanes/chat.md`.
//!
//! - [`xml`]: an incremental framer for the XML subset an XMPP stream uses
//!   ([`Framer`] in, [`Event`]s out) and an [`Element`] that serialises with
//!   no whitespace between tags -- which the client requires, not prefers.
//! - [`sasl`]: base64 and SASL PLAIN.
//! - [`client`]: a minimal client that sends the EVE client's own byte-exact
//!   templates. For tests, and for bots that want to talk in Local.
//!
//! Standard library only, like the rest of the workspace.

#![forbid(unsafe_code)]

pub mod client;
pub mod sasl;
pub mod xml;

pub use client::Client;
pub use xml::{Element, Event, Framer, XmlError, escape};
