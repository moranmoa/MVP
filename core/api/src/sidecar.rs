//! JSON lines over stdin/stdout, for running the core as a sidecar process
//! (Electron, decision 7). `src/bin/mvs-core.rs` wires it to the real ports;
//! tests drive [`serve`] in-process.
//!
//! - In: `{"id", "cmd", "args"}`, one per line. `id` is the caller's.
//! - Out: `{"type":"response","id",…"ok"|"error"}` and
//!   `{"type":"event","event","payload"}`, one per line. A request's events
//!   are written before its response.
//! - A line that isn't a request, or is over [`MAX_LINE`], gets an error
//!   response (`id` null unless it is a number or string), and serving
//!   goes on.

use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::ports::EventSink;
use crate::types::{CoreError, ErrorCode, Event};
use crate::Core;

pub use crate::datadir::prepare as prepare_data_dir;

/// Longest request line, in bytes, without its newline.
pub const MAX_LINE: usize = 1 << 20;

/// Where responses and events go, one JSON object per line.
pub struct Lines<W: Write>(Mutex<W>);

impl<W: Write> Lines<W> {
    pub fn new(out: W) -> Arc<Lines<W>> {
        Arc::new(Lines(Mutex::new(out)))
    }

    fn write(&self, value: &Value) -> io::Result<()> {
        let mut out = self.0.lock().expect("not poisoned");
        serde_json::to_writer(&mut *out, value)?;
        out.write_all(b"\n")?;
        out.flush()
    }

    /// The writer back, for tests.
    pub fn into_inner(self) -> W {
        self.0.into_inner().expect("not poisoned")
    }
}

/// Events go out on the same lines as responses.
impl<W: Write + Send> EventSink for Lines<W> {
    fn emit(&self, event: &Event) {
        let mut value = serde_json::to_value(event).expect("events serialize");
        value["type"] = json!("event");
        // A closed stdout means the shell is gone; `serve` sees it next.
        let _ = self.write(&value);
    }
}

fn response(id: &Value, result: Result<Value, CoreError>) -> Value {
    match result {
        Ok(ok) => json!({ "type": "response", "id": id, "ok": ok }),
        Err(error) => json!({ "type": "response", "id": id, "error": error }),
    }
}

/// One line, read up to `MAX_LINE` bytes: `Ok(None)` at end of input,
/// `Ok(Some(Err(())))` for a line that was too long (and is skipped).
fn read_line(input: &mut impl BufRead) -> io::Result<Option<Result<Vec<u8>, ()>>> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let chunk = input.fill_buf()?;
        if chunk.is_empty() {
            return Ok(if line.is_empty() && !too_long {
                None
            } else if too_long {
                Some(Err(()))
            } else {
                Some(Ok(line))
            });
        }
        let (part, done) = match chunk.iter().position(|b| *b == b'\n') {
            Some(i) => (&chunk[..i], Some(i + 1)),
            None => (chunk, None),
        };
        if !too_long {
            if line.len() + part.len() > MAX_LINE {
                too_long = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(part);
            }
        }
        let used = done.unwrap_or(chunk.len());
        input.consume(used);
        if done.is_some() {
            return Ok(Some(if too_long { Err(()) } else { Ok(line) }));
        }
    }
}

/// Answers requests from `input` until it ends. Returns how many lines it
/// answered; an error only when reading or writing fails.
pub fn serve<W: Write + Send>(
    core: &Core,
    mut input: impl BufRead,
    out: &Lines<W>,
) -> io::Result<u64> {
    let bad = || Err(CoreError::from(ErrorCode::BadRequest));
    let mut answered = 0;
    while let Some(line) = read_line(&mut input)? {
        let reply = match line {
            Err(()) => {
                eprintln!("mvs-core: refused a request over {MAX_LINE} bytes");
                response(&Value::Null, bad())
            }
            Ok(line) if line.iter().all(u8::is_ascii_whitespace) => continue,
            Ok(line) => match serde_json::from_slice::<Value>(&line) {
                Ok(Value::Object(mut request)) => {
                    // Only a number or string is echoed: anything else could
                    // be large or shaped to confuse the shell.
                    let id = match request.remove("id") {
                        Some(id @ (Value::Number(_) | Value::String(_))) => id,
                        _ => Value::Null,
                    };
                    let args = request.remove("args").unwrap_or(Value::Null);
                    match request.get("cmd").and_then(Value::as_str) {
                        Some(cmd) if !id.is_null() => response(&id, core.handle_value(cmd, args)),
                        _ => response(&id, bad()),
                    }
                }
                _ => response(&Value::Null, bad()),
            },
        };
        out.write(&reply)?;
        answered += 1;
    }
    Ok(answered)
}
