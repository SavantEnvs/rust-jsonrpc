# simple_http: infinite loop on unterminated HTTP response headers

**Target:** `simple_http` (SimpleHttpTransport HTTP/1.1 response parser)
**Type:** hang / infinite loop (denial of service)
**Reproducer:** `repro.http` — the 21 bytes `HTTP/1.1 200 OK\r\nX: y` (a valid
status line, one header line, and NO terminating blank `\r\n\r\n` before EOF).

## Cause

`SimpleHttpTransport::try_request` (`src/http/simple_http.rs`, ~line 207) parses
the response header fields with:

```rust
loop {
    header_buf.clear();
    sock.read_line(&mut header_buf)?;   // line 210
    if header_buf == "\r\n" {
        break;
    }
    ...
}
```

The loop's ONLY exit is a line that equals exactly `"\r\n"` (the blank line that
separates headers from body). When the underlying stream reaches end-of-file,
`BufRead::read_line` returns `Ok(0)` and leaves `header_buf` empty (`""`), which
is not `"\r\n"`, so the loop neither breaks nor errors — it calls `read_line`
again, which returns `Ok(0)` again immediately, forever. Any response whose
headers are not terminated by a blank line before the connection closes hangs the
client indefinitely, pinning a CPU. A malicious or buggy RPC server (or a
truncated/half-closed connection) triggers it.

## Impact

Denial of service in any consumer of `jsonrpc::simple_http`. The call never
returns; the default 1 s socket timeout does not help because the shim/stream
keeps yielding `Ok(0)` at EOF rather than blocking or erroring.

## One-line fix

Treat EOF as an error inside the loop, e.g.:

```rust
if sock.read_line(&mut header_buf)? == 0 {
    return Err(Error::HttpResponseTooShort { actual: 0, needed: 2 });
}
```

(i.e. bail when `read_line` returns `Ok(0)` instead of only breaking on `"\r\n"`).

## Reproduce

```
/mayhem/simple_http -timeout=8 repro.http     # → "libFuzzer: timeout", exit 70
```

## Harness note

`mayhem/fuzz/fuzz_targets/simple_http.rs` guards ONLY this precondition. Its
`hangs_on_unterminated_headers()` helper faithfully simulates `try_request`'s
line reader and status-line checks and skips an input **only** when that input
would actually enter the header loop (a valid `HTTP/1.1 <code>` status line) and
then run past EOF without a blank `"\r\n"` line — i.e. exactly the hanging inputs.
Everything else flows through, including the random, non-HTTP-looking inputs that
dominate a COLD corpus (they fail the status-line checks and drive the instrumented
status-line / header / content-length / body parser), so coverage bootstraps from
an empty corpus instead of starving. An earlier, too-broad guard rejected every
input lacking `\r\n\r\n` wholesale and pinned cold-corpus coverage at the harness
baseline (~34 edges); the narrowed guard lets random input reach the parser while
still keeping the DoS bounded. Remove the guard entirely once the upstream loop is
fixed. The reproducer is kept HERE, never in `testsuite/` (a hanging seed would
break every future run).
