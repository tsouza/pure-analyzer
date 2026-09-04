use std::io::{self, BufRead, Read, Write};

use serde_json::Value;

/// Largest frame body this reader will allocate for.
///
/// `Content-Length` is attacker-controlled and picks an allocation size, so an
/// unbounded value aborts the process instead of failing the connection. 64 MiB
/// clears the largest realistic frame — a full-sync `didChange` carrying one
/// document as an escaped JSON string — by orders of magnitude.
const MAX_CONTENT_LENGTH_BYTES: usize = 64 * 1024 * 1024;

/// Largest header line, including its terminator.
///
/// LSP defines only `Content-Length` and `Content-Type`; a line beyond this is
/// a client that will never send the `\n` this reader is waiting for.
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;

pub(crate) fn read_frame<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut content_length = None;
    let mut saw_header = false;
    loop {
        let mut line = Vec::new();
        let read = read_header_line(reader, &mut line)?;
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
            content_length = Some(content_length_value(value)?);
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

/// Reads one header line, refusing a line that outruns [`MAX_HEADER_LINE_BYTES`].
///
/// A short read means the stream ended; only a read that fills the cap without
/// reaching a terminator is an over-long line.
fn read_header_line<R: BufRead>(reader: &mut R, line: &mut Vec<u8>) -> io::Result<usize> {
    let read = reader
        .by_ref()
        .take(MAX_HEADER_LINE_BYTES as u64)
        .read_until(b'\n', line)?;
    if read == MAX_HEADER_LINE_BYTES && !line.ends_with(b"\n") {
        return Err(invalid("LSP header line exceeds maximum length"));
    }
    Ok(read)
}

fn content_length_value(value: &str) -> io::Result<usize> {
    let length: usize = value
        .parse()
        .map_err(|_| invalid("invalid Content-Length"))?;
    if length > MAX_CONTENT_LENGTH_BYTES {
        return Err(invalid("Content-Length exceeds maximum frame size"));
    }
    Ok(length)
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
    use std::io::{self, BufReader, Cursor, ErrorKind};

    use serde_json::Value;

    use super::{
        MAX_CONTENT_LENGTH_BYTES, MAX_HEADER_LINE_BYTES, content_length_value, read_frame,
        write_frame,
    };

    fn value(source: &str) -> Value {
        serde_json::from_str(source).expect("test JSON must parse")
    }

    #[test]
    fn empty_input_has_no_frame() {
        assert_eq!(read_frame(&mut Cursor::new([])).expect("empty input"), None);
    }

    #[test]
    fn absurd_content_length_is_reported_not_allocated() {
        let error = read_frame(&mut Cursor::new(b"Content-Length: 4000000000000\r\n\r\n"))
            .expect_err("an unsatisfiable Content-Length must be an error");
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("Content-Length exceeds"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn endless_header_line_is_reported_not_read_forever() {
        let error = read_frame(&mut BufReader::new(io::repeat(b'x')))
            .expect_err("a header line with no terminator must be an error");
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("header line exceeds"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn truncated_header_line_keeps_its_end_of_stream_error() {
        let error = read_frame(&mut Cursor::new(b"Content-Length: 2"))
            .expect_err("a truncated header must be an error");
        assert!(
            error.to_string().contains("unexpected end of LSP headers"),
            "a short final line is a truncated stream, not an over-long line: {error}"
        );
    }

    #[test]
    fn content_length_is_bounded_at_the_maximum() {
        assert_eq!(
            content_length_value(&MAX_CONTENT_LENGTH_BYTES.to_string())
                .expect("the maximum itself is accepted"),
            MAX_CONTENT_LENGTH_BYTES
        );
        let error = content_length_value(&(MAX_CONTENT_LENGTH_BYTES + 1).to_string())
            .expect_err("one byte past the maximum is rejected");
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn header_line_at_the_maximum_is_accepted() {
        let padding = MAX_HEADER_LINE_BYTES - b"X-Pad: \r\n".len();
        let frame = format!(
            "X-Pad: {}\r\nContent-Length: 2\r\n\r\n{{}}",
            "p".repeat(padding)
        );
        assert_eq!(
            read_frame(&mut Cursor::new(frame)).expect("a header line at the cap is accepted"),
            Some(value("{}"))
        );
    }

    #[test]
    fn max_content_length_bytes_is_64_mebibytes() {
        // Recomputed independently of the constant's own definition, so a
        // mutated `*` there (e.g. `64 + 1024 * 1024`) changes the value
        // this compares against. Written as the same `*` expression (not a
        // pre-computed decimal literal) to keep clippy's
        // `decimal_literal_representation` lint quiet.
        assert_eq!(MAX_CONTENT_LENGTH_BYTES, 64 * 1024 * 1024);
    }

    #[test]
    fn max_header_line_bytes_is_8_kibibytes() {
        assert_eq!(MAX_HEADER_LINE_BYTES, 8 * 1024);
    }

    #[test]
    fn written_frame_reads_back() {
        let payload = value(r#"{"jsonrpc":"2.0","method":"initialized"}"#);
        let mut buffer = Vec::new();
        write_frame(&mut buffer, &payload).expect("write");
        assert_eq!(
            read_frame(&mut Cursor::new(buffer)).expect("read"),
            Some(payload)
        );
    }
}
