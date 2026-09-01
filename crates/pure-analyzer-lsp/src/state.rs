use serde_json::Value;

use crate::{DocumentSnapshot, RequestId, Server};

pub(crate) fn cancel(server: &mut Server, params: Option<&Value>) {
    if let Some(request) = params
        .and_then(|value| value.get("id"))
        .and_then(RequestId::from_json)
    {
        server.cancellation.cancel(request);
    }
}

pub(crate) fn open_document(server: &mut Server, params: Option<&Value>) {
    let Some(document) = params.and_then(|value| value.get("textDocument")) else {
        return;
    };
    let (Some(uri), Some(text)) = (
        document.get("uri").and_then(Value::as_str),
        document.get("text").and_then(Value::as_str),
    ) else {
        return;
    };
    server.documents.insert(DocumentSnapshot::new(
        uri.to_owned(),
        text.to_owned(),
        document.get("version").and_then(Value::as_i64),
    ));
}

pub(crate) fn close_document(server: &mut Server, params: Option<&Value>) {
    if let Some(uri) = params
        .and_then(|value| value.get("textDocument"))
        .and_then(|value| value.get("uri"))
        .and_then(Value::as_str)
    {
        let _ = server.documents.remove(uri);
    }
}

pub(crate) fn update_configuration(server: &mut Server, params: Option<&Value>) {
    if let Some(settings) = params.and_then(|value| value.get("settings")).cloned() {
        server.configuration.replace(settings);
    }
}
