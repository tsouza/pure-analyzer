use std::io::{self, Write};

use serde_json::{Map, Value};

use crate::frame::write_frame;

pub(crate) fn send_result<W: Write>(writer: &mut W, id: Value, result: Value) -> io::Result<()> {
    write_frame(
        writer,
        &object([
            ("jsonrpc", Value::String("2.0".to_owned())),
            ("id", id),
            ("result", result),
        ]),
    )
}

pub(crate) fn send_error<W: Write>(
    writer: &mut W,
    id: Value,
    code: i64,
    message: &str,
) -> io::Result<()> {
    let error = object([
        ("code", Value::Number(code.into())),
        ("message", Value::String(message.to_owned())),
    ]);
    write_frame(
        writer,
        &object([
            ("jsonrpc", Value::String("2.0".to_owned())),
            ("id", id),
            ("error", error),
        ]),
    )
}

pub(crate) fn initialization_result() -> Value {
    let server_info = object([
        ("name", Value::String("pure-analyzer-lsp".to_owned())),
        (
            "version",
            Value::String(env!("CARGO_PKG_VERSION").to_owned()),
        ),
    ]);
    object([
        ("capabilities", Value::Object(Map::new())),
        ("serverInfo", server_info),
    ])
}

fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}
