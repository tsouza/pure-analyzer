use std::io::{self, Write};

use serde_json::Value;

use crate::{
    Server, ServerExit,
    response::{initialization_result, send_error, send_result},
    scheduler::{RequestScheduler, ScheduleResult},
    server::Lifecycle,
    state::{self, RequestWork},
};

const INVALID_REQUEST_CODE: i64 = -32_600;
const METHOD_NOT_FOUND_CODE: i64 = -32_601;
const INVALID_PARAMS_CODE: i64 = -32_602;

pub(crate) fn handle<W: Write>(
    server: &mut Server,
    message: Value,
    writer: &mut W,
    scheduler: &mut RequestScheduler,
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
        Some("textDocument/hover") => hover(server, id, params, writer, scheduler),
        Some("workspace/didChangeConfiguration") => {
            state::update_configuration(server, params, writer)?;
            Ok(None)
        }
        Some("textDocument/definition") => definition(server, id, params, writer, scheduler),
        Some("textDocument/codeAction") => code_action(server, id, params, writer, scheduler),
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
    scheduler: &mut RequestScheduler,
) -> io::Result<Option<ServerExit>> {
    dispatch_work(
        server,
        id,
        state::hover_work(server, params),
        writer,
        scheduler,
    )
}

fn definition<W: Write>(
    server: &Server,
    id: Option<Value>,
    params: Option<&Value>,
    writer: &mut W,
    scheduler: &mut RequestScheduler,
) -> io::Result<Option<ServerExit>> {
    dispatch_work(
        server,
        id,
        state::definition_work(server, params),
        writer,
        scheduler,
    )
}

fn code_action<W: Write>(
    server: &Server,
    id: Option<Value>,
    params: Option<&Value>,
    writer: &mut W,
    scheduler: &mut RequestScheduler,
) -> io::Result<Option<ServerExit>> {
    dispatch_work(
        server,
        id,
        state::code_actions_work(server, params),
        writer,
        scheduler,
    )
}

/// Schedules `work`, or replies with `-32602 invalid params` when the
/// request's parameters could not be turned into work in the first place.
///
/// Shared by every request kind so a malformed request yields the same
/// protocol error regardless of which handler received it.
fn dispatch_work<W: Write>(
    server: &Server,
    id: Option<Value>,
    work: Result<RequestWork, state::RequestParamsError>,
    writer: &mut W,
    scheduler: &mut RequestScheduler,
) -> io::Result<Option<ServerExit>> {
    let Some(id) = id else {
        return Ok(None);
    };
    match work {
        Ok(work) => schedule(server, id, work, writer, scheduler)?,
        Err(state::RequestParamsError::InvalidParams) => {
            send_error(writer, id, INVALID_PARAMS_CODE, "invalid params")?;
        }
    }
    Ok(None)
}

fn schedule<W: Write>(
    server: &Server,
    id: Value,
    work: RequestWork,
    writer: &mut W,
    scheduler: &mut RequestScheduler,
) -> io::Result<()> {
    match scheduler.schedule(server, id, work)? {
        ScheduleResult::Scheduled => Ok(()),
        ScheduleResult::DuplicateIdentifier(id) => send_error(
            writer,
            id,
            INVALID_REQUEST_CODE,
            "duplicate active request id",
        ),
    }
}

fn exit(server: &Server) -> ServerExit {
    if server.lifecycle == Lifecycle::ShuttingDown {
        ServerExit::Clean
    } else {
        ServerExit::Unclean
    }
}
