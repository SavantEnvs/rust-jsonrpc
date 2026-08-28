// KAT (known-answer test) oracle probe for the rust-jsonrpc Mayhem integration.
//
// Drives jsonrpc's PUBLIC API on FIXED inputs and prints exact, deterministic
// values. mayhem/test.sh runs this binary and greps the exact expected lines
// (SPEC §6.3 anti-reward-hacking): a PATCH that neuters the library (or this
// probe) changes/eliminates the output and the oracle FAILS.
//
// KAT2/KAT3 go END-TO-END through SimpleHttpTransport's hand-rolled HTTP/1.1
// response parser — the exact code path the `simple_http` fuzz target explores —
// using the library's own `--cfg jsonrpc_fuzz` FUZZ_TCP_SOCK injection point to
// serve a canned response. No file I/O, no real network.

use std::io;

use jsonrpc::simple_http::{SimpleHttpTransport, FUZZ_TCP_SOCK};
use jsonrpc::{Client, Response};

fn fresh_client() -> Client {
    let t = SimpleHttpTransport::builder()
        .url("localhost:123")
        .expect("parse url")
        .auth("", None)
        .build();
    Client::with_transport(t)
}

fn serve(body: &str) {
    let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
    *FUZZ_TCP_SOCK.lock().unwrap() = Some(io::Cursor::new(resp.into_bytes()));
}

fn main() {
    // KAT1: request construction + serialization. A fresh client's first request
    // must carry id=1 and jsonrpc="2.0" (JSON-RPC 2.0 framing).
    let client = fresh_client();
    let req = client.build_request("uptime", None);
    println!("KAT1 request={}", serde_json::to_string(&req).expect("KAT1 serialize failed"));

    // KAT2: full round trip through SimpleHttpTransport's HTTP response parser:
    // status line, content-length header, serde_json body, id/version checks.
    let client = fresh_client();
    serve(r#"{"jsonrpc":"2.0","result":31415926,"id":1}"#);
    let uptime: u64 = client.call("uptime", None).expect("KAT2 call failed");
    println!("KAT2 uptime={}", uptime);

    // KAT3: a response whose id does NOT match the request nonce must be REJECTED
    // (an oracle for the error path, so a client that "accepts everything" fails).
    let client = fresh_client();
    serve(r#"{"jsonrpc":"2.0","result":0,"id":999}"#);
    let mismatch = matches!(client.call::<u64>("uptime", None), Err(jsonrpc::Error::NonceMismatch));
    println!("KAT3 nonce_mismatch={}", mismatch);

    // KAT4: Response deserialization + typed result extraction (RawValue path).
    let resp: Response = serde_json::from_str(r#"{"jsonrpc":"2.0","result":["hello",5],"id":9}"#)
        .expect("KAT4 parse failed");
    let (s, n): (String, u64) = resp.result().expect("KAT4 result failed");
    println!("KAT4 s={} n={}", s, n);

    // KAT5: a JSON-RPC error object must surface as Error::Rpc with the exact code.
    let resp: Response = serde_json::from_str(
        r#"{"jsonrpc":"2.0","error":{"code":-32601,"message":"Method not found"},"id":3}"#,
    )
    .expect("KAT5 parse failed");
    match resp.result::<u64>() {
        Err(jsonrpc::Error::Rpc(e)) => println!("KAT5 code={}", e.code),
        other => println!("KAT5 unexpected={:?}", other.is_ok()),
    }
}
