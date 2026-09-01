use std::io::{self, BufRead, Write};

use serde_json::Value;

pub(crate) fn read_frame<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut content_length = None;
    let mut saw_header = false;
    loop {
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            return if saw_header {
                Err(invalid("unexpected end of LSP headers"))
            } else {
                Ok(None)
            };
        }
        saw_header = true;
        let line = trim_line(&line);
        if line.is_empty() {
            break;
        }
        let (name, value) = header_parts(line).ok_or_else(|| invalid("malformed LSP header"))?;
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .parse()
                    .map_err(|_| invalid("invalid Content-Length"))?,
            );
        }
    }
    let length = content_length.ok_or_else(|| invalid("missing Content-Length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(invalid_json)
}

pub(crate) fn write_frame<W: Write>(writer: &mut W, value: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(value).map_err(invalid_json)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn trim_line(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\n")
        .unwrap_or(line)
        .strip_suffix(b"\r")
        .unwrap_or(line)
}

fn header_parts(line: &[u8]) -> Option<(&str, &str)> {
    let line = std::str::from_utf8(line).ok()?;
    let (name, value) = line.split_once(':')?;
    Some((name.trim(), value.trim()))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn invalid_json(error: impl std::fmt::Display) -> io::Error {
    invalid(format!("invalid JSON-RPC payload: {error}"))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::read_frame;

    #[test]
    fn empty_input_has_no_frame() {
        assert_eq!(read_frame(&mut Cursor::new([])).expect("empty input"), None);
    }
}
