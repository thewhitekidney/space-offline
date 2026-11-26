//! Node graphs on the server: the client's node-graph engine, run here.
//!
//! The client ships both halves of its scripted flows as node graphs in its
//! static data (`docs/re/node-graphs.md`): `server`-tagged graphs, which the
//! server runs, and `client`-tagged ones, which the server starts on the
//! client and talks to with `OnStartClientNodeGraph` /
//! `OnClientNodeGraphMessage`. This module runs the server half:
//!
//! - [`value`]: Python values and `compare_values`, as the client compares.
//! - [`data`]: the graphs, node types and atoms, loaded from the bundle's
//!   `nodegraphs/` directory.
//! - [`engine`]: one root graph instance and its subgraphs, as a
//!   [`engine::Runtime`] that returns [`engine::Effect`]s for the caller to
//!   deliver.

pub mod data;
pub mod engine;
pub mod value;
