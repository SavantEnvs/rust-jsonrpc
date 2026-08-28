//! libFuzzer port of upstream fuzz/fuzz_targets/simple_http.rs (a honggfuzz harness).
//!
//! The fuzz bytes are injected as the READ side of the transport's TCP socket via the
//! library's own `--cfg jsonrpc_fuzz` hook (`FUZZ_TCP_SOCK`), so every input is parsed
//! as a raw HTTP/1.1 response by SimpleHttpTransport's hand-rolled HTTP client
//! (status line, headers, content-length, then serde_json body). No file I/O, no
//! real network: under jsonrpc_fuzz the library's TcpStream is a Cursor shim.
#![no_main]

use std::io;

use jsonrpc::simple_http::{SimpleHttpTransport, FUZZ_TCP_SOCK};
use jsonrpc::Client;
use libfuzzer_sys::fuzz_target;

/// One `BufRead::read_line` outcome, modelled over the fuzz cursor:
/// `Eof` = `Ok(0)` (empty buffer at end-of-stream); `Invalid` = `Err(InvalidData)`
/// (the line's bytes were consumed but are not UTF-8); `Line` = `Ok(n>0)` with the
/// line bytes, INCLUDING the trailing `\n` (the final chunk may lack one).
enum LineRes<'a> {
    Line(&'a [u8]),
    Invalid,
    Eof,
}

/// `read_line` semantics: consume `data[*pos..]` up to and including the next `\n`
/// (or to EOF), advancing `*pos`. Bytes are consumed even on a UTF-8 error, exactly
/// like `std`'s `read_until` + UTF-8 validation.
fn read_line<'a>(data: &'a [u8], pos: &mut usize) -> LineRes<'a> {
    if *pos >= data.len() {
        return LineRes::Eof; // Ok(0), empty buffer
    }
    let rest = &data[*pos..];
    let end = rest.iter().position(|&b| b == b'\n').map_or(rest.len(), |i| i + 1);
    let line = &rest[..end];
    *pos += end; // bytes are consumed regardless of UTF-8 validity
    if std::str::from_utf8(line).is_err() {
        LineRes::Invalid
    } else {
        LineRes::Line(line)
    }
}

/// Faithfully simulates the *only* infinite-loop precondition in
/// `SimpleHttpTransport::try_request` (src/http/simple_http.rs, see
/// mayhem/simple_http/known-findings/unterminated-headers-infinite-loop/):
/// after a valid status line the header-parsing loop breaks ONLY on a line equal
/// to `"\r\n"`; at EOF `BufRead::read_line` returns `Ok(0)` with an empty buffer
/// (`""` != `"\r\n"`), so a response that ENTERS that loop but is never terminated
/// by a blank line before end-of-stream spins forever.
///
/// Returns true for exactly those hanging inputs and NOTHING else, so the vast
/// majority of random fuzzer inputs (which fail the status-line checks) flow
/// through and drive the instrumented parser — that is what lets coverage
/// bootstrap from a COLD (empty) corpus. Remove this guard once the upstream loop
/// is fixed.
///
/// The status line is subtle: try_request reads the first line with `.is_ok()`
/// (NOT `?`), so a non-UTF-8 OR empty first line does NOT return — it triggers a
/// one-shot retry (`fresh_socket`, which under the fuzz shim does NOT reset the
/// cursor) that reads the *next* line, with `?`, as the status line. We model that
/// retry so an input whose leading bytes are non-UTF-8 but whose SECOND line is a
/// valid `HTTP/1.1 <code>` status line is still recognised as a hang.
fn hangs_on_unterminated_headers(data: &[u8]) -> bool {
    let mut pos = 0usize;

    // (1) Acquire the status line, mirroring try_request lines ~175-186.
    // Under the fuzz shim writes go to a sink, so `write_success` is always true.
    let status: &[u8] = match read_line(data, &mut pos) {
        // A non-empty, valid first line IS the status line (no retry).
        LineRes::Line(l) => l,
        // First read failed (non-UTF-8) or was empty (EOF): retry, reading the NEXT
        // line with `?`. A non-UTF-8 retry line propagates Err -> try_request returns
        // (no hang); EOF yields an empty status buffer (rejected by the len check).
        LineRes::Invalid | LineRes::Eof => match read_line(data, &mut pos) {
            LineRes::Line(l) => l,
            LineRes::Eof => b"",
            LineRes::Invalid => return false,
        },
    };
    if status.len() < 12
        || !status[..12].is_ascii()
        || !status.starts_with(b"HTTP/1.1 ")
        // bytes [9..12] are ASCII (checked above); must parse as a u16 status code
        || std::str::from_utf8(&status[9..12]).unwrap().parse::<u16>().is_err()
    {
        return false; // status line rejected -> try_request returns, no hang
    }

    // (2) Header loop — hangs iff it reaches EOF without ever seeing a blank line.
    loop {
        match read_line(data, &mut pos) {
            LineRes::Line(b"\r\n") => return false, // loop breaks: terminated, no hang
            LineRes::Line(_) => continue,           // ordinary header line
            LineRes::Invalid => return false,       // read_line? errors out -> returns
            LineRes::Eof => return true,            // EOF before a blank line -> HANG
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // HANG GUARD (upstream bug, see mayhem/simple_http/known-findings/):
    // Skip ONLY inputs that would spin try_request's header loop forever (a valid
    // status line whose header section never terminates before EOF). Every other
    // input — including the random, non-HTTP-looking inputs that dominate a cold
    // corpus — flows through and exercises the instrumented parser, so coverage
    // bootstraps from an empty corpus. The reproducer + one-line fix are recorded
    // under known-findings/.
    if hangs_on_unterminated_headers(data) {
        return;
    }

    *FUZZ_TCP_SOCK.lock().unwrap() = Some(io::Cursor::new(data.to_vec()));

    let t = SimpleHttpTransport::builder()
        .url("localhost:123")
        .expect("parse url")
        .auth("", None)
        .build();

    let client = Client::with_transport(t);
    let request = client.build_request("uptime", None);
    let _ = client.send_request(request);
});
