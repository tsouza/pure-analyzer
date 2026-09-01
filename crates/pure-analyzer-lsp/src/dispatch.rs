use std::io::{self, Write};

use serde_json::Value;

use crate::{
    Server, ServerExit,
    response::{initialization_result, send_error, send_result},
    server::Lifecycle,
    state,
};

const INVALID_REQUEST_CODE: i64 = -32_600;
const METHOD_NOT_FOUND_CODE: i64 = -32_601;
const INVALID_PARAMS_CODE: i64 = -32_602;

pub(crate) fn handle<W: Write>(
    server: &mut Server,
    message: Value,
    writer: &mut W,
) -> io::Result<Option<ServerExit>> {
    let Some(object) = message.as_object() else {
        send_error(writer, Value::Null, INVALID_REQUEST_CODE, "invalid request")?;
        return Ok(None);
    };
    let method = object.get("method").and_then(Value::as_str);
    let id = object.get("id").cloned();
    let params = object.get("params");
    match method {
        Some("initialize") => initialize(server, id, writer),
        Some("shutdown") => shutdown(server, id, writer),
        Some("exit") => Ok(Some(exit(server))),
        Some("$/cancelRequest") => {
            state::cancel(server, params);
            Ok(None)
        }
        Some("textDocument/didOpen") => {
            state::open_document(server, params, writer)?;
            Ok(None)
        }
        Some("textDocument/didChange") => {
            state::change_document(server, params, writer)?;
            Ok(None)
        }
        Some("textDocument/didSave") => {
            state::save_document(server, params, writer)?;
            Ok(None)
        }
        Some("textDocument/didClose") => {
            state::close_document(server, params, writer)?;
            Ok(None)
        }
        Some("textDocument/hover") => hover(server, id, params, writer),
        Some("workspace/didChangeConfiguration") => {
            state::update_configuration(server, params, writer)?;
            Ok(None)
        }
        Some("textDocument/definition") => {
            if let Some(id) = id {
                send_result(writer, id, state::definition(server, params))?;
            }
            Ok(None)
        }
        Some(_) if let Some(id) = id => {
            send_error(writer, id, METHOD_NOT_FOUND_CODE, "method not found")?;
            Ok(None)
        }
        Some(_) => Ok(None),
        None if let Some(id) = id => {
            send_error(writer, id, INVALID_REQUEST_CODE, "invalid request")?;
            Ok(None)
        }
        None => Ok(None),
    }
}

fn initialize<W: Write>(
    server: &mut Server,
    id: Option<Value>,
    writer: &mut W,
) -> io::Result<Option<ServerExit>> {
    let Some(id) = id else {
        return Ok(None);
    };
    if server.lifecycle != Lifecycle::New {
        send_error(
            writer,
            id,
            INVALID_REQUEST_CODE,
            "initialize may only run once",
        )?;
        return Ok(None);
    }
    server.lifecycle = Lifecycle::Running;
    send_result(writer, id, initialization_result())?;
    Ok(None)
}

fn shutdown<W: Write>(
    server: &mut Server,
    id: Option<Value>,
    writer: &mut W,
) -> io::Result<Option<ServerExit>> {
    let Some(id) = id else {
        return Ok(None);
    };
    if server.lifecycle != Lifecycle::Running {
        send_error(
            writer,
            id,
            INVALID_REQUEST_CODE,
            "shutdown requires initialize",
        )?;
        return Ok(None);
    }
    server.lifecycle = Lifecycle::ShuttingDown;
    send_result(writer, id, Value::Null)?;
    Ok(None)
}

fn hover<W: Write>(
    server: &Server,
    id: Option<Value>,
    params: Option<&Value>,
    writer: &mut W,
) -> io::Result<Option<ServerExit>> {
    let Some(id) = id else {
        return Ok(None);
    };
    match state::hover(server, params) {
        Ok(result) => send_result(writer, id, result.unwrap_or(Value::Null))?,
        Err(state::HoverError::InvalidParams) => {
            send_error(writer, id, INVALID_PARAMS_CODE, "invalid params")?;
        }
    }
    Ok(None)
}

fn exit(server: &Server) -> ServerExit {
    if server.lifecycle == Lifecycle::ShuttingDown {
        ServerExit::Clean
    } else {
        ServerExit::Unclean
    }
}
