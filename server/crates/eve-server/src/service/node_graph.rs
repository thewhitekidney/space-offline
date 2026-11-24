//! `node_graph`: the client's node-graph service talking to the server graph
//! that started it. The wire arms only; the runtime is `crate::air_npe`.
//!
//! Both calls name the server graph by its instance id, the
//! `server_graph_instance_id` the server put in `OnStartClientNodeGraph`'s
//! blackboard **[V]** (`nodegraph/client/graph.py`, decompiled 3707965:
//! `process_client_graph_message(graph_id, server_graph_instance_id, key, value)`,
//! `process_client_blackboard_request(graph_id, server_graph_instance_id, key)`,
//! whose return value is the answer).

use super::*;
use crate::air_npe;

/// The services dispatched here.
pub(crate) const SERVICES: &[&str] = &["node_graph"];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let ar = args::Args::new(req);
    match req.method {
        "process_client_blackboard_request" => {
            let (Some(instance), Some(key)) = (ar.text(1), ar.text(2)) else {
                return Err(ar.expected("(graph_id, server_graph_instance_id, key)"));
            };
            Ok(air_npe::blackboard_request(reg, req.actor, &instance, &key))
        }
        "process_client_graph_message" => {
            let (Some(instance), Some(key)) = (ar.text(1), ar.text(2)) else {
                return Err(ar.expected("(graph_id, server_graph_instance_id, key, value)"));
            };
            let value = ar
                .get(3)
                .map_or(crate::node_graph::value::Value::None, air_npe::from_py);
            air_npe::graph_message(reg, req.actor, &instance, &key, value);
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}
