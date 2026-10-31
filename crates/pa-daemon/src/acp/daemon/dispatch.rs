//! Frame dispatch: one incoming request routed to its handler, and the
//! `initialize` handshake with the TS SDK's protocol-version schema check.

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::link::DaemonLink;
use super::session_new::handle_session_new;
use super::{DaemonAcpOptions, DaemonAcpState, DaemonBinding};
use crate::acp::jsonrpc::{self, Incoming};
use crate::acp::producer;
use crate::acp::types;
use crate::acp::wire_config::handle_set_config_option;

/// One incoming ACP frame. Requests answer.
pub(super) async fn handle_incoming(
    incoming: Incoming,
    link: &Arc<DaemonLink>,
    state: &Arc<Mutex<DaemonAcpState>>,
    options: &DaemonAcpOptions,
    binding: &DaemonBinding,
    tx: producer::FrameSink,
) {
    let Incoming::Request { id, method, params } = incoming else {
        return;
    };
    match method.as_str() {
        "initialize" => {
            handle_initialize(&id, &params, &options.product_version, &tx);
        }
        "session/new" => {
            handle_session_new(id, params, link, state, options, binding, tx).await;
        }
        "session/set_config_option" => {
            handle_set_config_option(id, params, link, state, tx).await;
        }
        other => {
            let _ = tx.send(jsonrpc::error_response(
                &id,
                jsonrpc::METHOD_NOT_FOUND,
                &format!("\"Method not found\": {other}"),
                Some(&json!({ "method": other })),
            ));
        }
    }
}

fn handle_initialize(id: &Value, params: &Value, product_version: &str, tx: &producer::FrameSink) {
    if let Err(error_response) = validate_initialize(id, params) {
        let _ = tx.send(error_response);
        return;
    }
    let result =
        serde_json::to_value(types::initialize_result(product_version)).expect("serializes");
    let _ = tx.send(jsonrpc::response(id, &result));
}

/// The `initialize` schema check the TS SDK performs: the protocol version
/// must be a number. The error body mirrors the observed TS response.
fn validate_initialize(id: &Value, params: &Value) -> std::result::Result<(), Value> {
    let field_error = |received: &str| {
        jsonrpc::error_response(
            id,
            jsonrpc::INVALID_PARAMS,
            "Invalid params",
            Some(&json!({
                "_errors": [],
                "protocolVersion": {
                    "_errors": [format!("Invalid input: expected number, received {received}")]
                },
            })),
        )
    };
    match params.get("protocolVersion") {
        None => Err(field_error("undefined")),
        Some(value) if value.is_number() => Ok(()),
        Some(Value::String(_)) => Err(field_error("string")),
        Some(Value::Bool(_)) => Err(field_error("boolean")),
        Some(Value::Null) => Err(field_error("null")),
        Some(_) => Err(field_error("object")),
    }
}
