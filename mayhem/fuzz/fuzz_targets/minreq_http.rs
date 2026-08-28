//! libFuzzer port of upstream fuzz/fuzz_targets/minreq_http.rs (a honggfuzz harness).
//!
//! Same body as upstream: the fuzz bytes are loaded into the library's
//! `minreq_http::FUZZ_TCP_SOCK` injection point and a full JSON-RPC request is
//! round-tripped through the MinreqHttpTransport (request serialization via
//! serde_json, minreq URL/request handling, response/error paths).
#![no_main]

use std::io;

use jsonrpc::minreq_http::{MinreqHttpTransport, FUZZ_TCP_SOCK};
use jsonrpc::Client;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    *FUZZ_TCP_SOCK.lock().unwrap() = Some(io::Cursor::new(data.to_vec()));

    let t = MinreqHttpTransport::builder()
        .url("localhost:123")
        .expect("parse url")
        .basic_auth("".to_string(), None)
        .build();

    let client = Client::with_transport(t);
    let request = client.build_request("uptime", None);
    let _ = client.send_request(request);
});
