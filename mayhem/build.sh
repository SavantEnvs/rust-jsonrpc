#!/usr/bin/env bash
#
# mayhem/build.sh — build rust-jsonrpc's fuzz targets as sanitized libFuzzer
# binaries (OSS-Fuzz Rust path: cargo-fuzz + ASan via RUSTFLAGS), plus a small
# dynamically-linked KAT probe binary that mayhem/test.sh runs as the behavioral
# oracle, plus a precompile of the project's own test suite.
#
# Runs inside the commit image (RUST mayhem/Dockerfile) as `mayhem` in /mayhem.
# The Rust toolchain + cargo registry live at $CARGO_HOME=/opt/toolchains/rust/cargo
# (pinned by the Dockerfile ENV — absolute, $HOME-independent).
#
# AIR-GAPPED CONTRACT (SPEC §6.5): the PATCH tier re-runs THIS script OFFLINE.
#   - This FIRST build (online) populates the cargo registry under $CARGO_HOME.
#   - The PATCH re-run resolves crates from that cache (the runtime exports
#     CARGO_NET_OFFLINE=true), so we do NOT hard-code `--offline` here.
#   - Every dependency is pinned by committed lockfiles: the root build reuses
#     upstream's own Cargo-recent.lock; mayhem/fuzz and mayhem/kat commit theirs.
#
# Upstream's fuzz/ crate is a HONGGFUZZ harness (not libFuzzer), so we ship an
# ADDITIVE mayhem/fuzz/ cargo-fuzz crate (its own [workspace] table) exposing the
# SAME two targets — `simple_http` and `minreq_http` — with the harness bodies
# ported verbatim against the unmodified jsonrpc library. Nothing upstream is
# touched. The whole fuzz build carries upstream's own `--cfg jsonrpc_fuzz`, which
# swaps the transports' TcpStream for the FUZZ_TCP_SOCK cursor shim (no network).
set -euo pipefail

# clang rejects SOURCE_DATE_EPOCH='' — must be unset or a valid integer.
[ -n "${SOURCE_DATE_EPOCH:-}" ] || unset SOURCE_DATE_EPOCH

: "${MAYHEM_JOBS:=$(nproc)}"
# cargo-fuzz has no --jobs flag; cargo reads parallelism from CARGO_BUILD_JOBS.
export CARGO_BUILD_JOBS="$MAYHEM_JOBS"

# The toolchain the Dockerfile installed. Referenced EXPLICITLY (+$RUST_CHANNEL) on
# every cargo invocation so nothing (e.g. a future upstream rust-toolchain.toml)
# can silently hijack the channel. Already installed → rustup never hits the net.
RUST_CHANNEL="${RUST_CHANNEL:-nightly-2025-06-01}"

cd "$SRC"

TRIPLE="x86_64-unknown-linux-gnu"

# Upstream gitignores Cargo.lock but ships two pinned lockfiles; its own CI does
# exactly this copy. Pins the root/test dep tree deterministically (idempotent:
# same content on every re-run), so the offline re-run never resolves versions.
cp Cargo-recent.lock Cargo.lock

# ── 1. KAT probe (the behavioral oracle) — CLEAN build, no sanitizer / no DWARF pin ──
# A normal dynamically-linked Rust binary. Built with ONLY upstream's own
# `--cfg jsonrpc_fuzz` (which exposes the FUZZ_TCP_SOCK injection point so the probe
# can serve a canned HTTP response through the real SimpleHttpTransport parser); no
# sanitizer, no fuzzing cfg — an honest oracle build. test.sh asserts its exact
# output; the gate's sabotage shim neuters it (empty output => oracle FAILS).
echo "=== building KAT probe (clean, dynamically linked) ==="
env -u CFLAGS -u CXXFLAGS RUSTFLAGS="--cfg jsonrpc_fuzz" \
  cargo +"${RUST_CHANNEL}" build --release --manifest-path mayhem/kat/Cargo.toml
KAT_BIN="$SRC/mayhem/kat/target/release/jsonrpc-kat"
[ -x "$KAT_BIN" ] || { echo "ERROR: KAT probe not built at $KAT_BIN" >&2; exit 1; }
cp "$KAT_BIN" /mayhem/jsonrpc-kat
# Regression guard: the oracle only works if the probe is dynamically linked (so the
# gate's LD_PRELOAD sabotage shim can neuter it). A static binary would silently defeat it.
if ! file /mayhem/jsonrpc-kat | grep -q 'dynamically linked'; then
  echo "ERROR: KAT probe is not dynamically linked — the sabotage oracle would be defeated" >&2
  file /mayhem/jsonrpc-kat >&2
  exit 1
fi
echo "built /mayhem/jsonrpc-kat (dynamically linked)"

# ── 2. Precompile the project's own test suite with NORMAL flags (clean build) ──
# so mayhem/test.sh only RUNS it. Default features (simple_http, simple_tcp);
# pinned by the copied Cargo.lock. Root package only — the workspace's fuzz/
# (honggfuzz) and integration_test members are not built.
echo "=== building jsonrpc test suite (cargo test --no-run) ==="
env -u RUSTFLAGS -u CFLAGS -u CXXFLAGS cargo +"${RUST_CHANNEL}" test --no-run

# ── 3. Sanitized libFuzzer targets via cargo-fuzz ─────────────────────────────
# Sanitizers (§6.1): the base provides clang $SANITIZER_FLAGS (ASan+UBSan, halting).
# rustc can't consume those clang flags, but we honor the KNOB: non-empty
# $SANITIZER_FLAGS => instrument the Rust build with ASan (the OSS-Fuzz Rust path);
# an explicit empty `--build-arg SANITIZER_FLAGS=` yields an un-sanitized build.
RUST_SAN=""
if [ -n "${SANITIZER_FLAGS:-}" ]; then
  RUST_SAN="-Zsanitizer=address"
fi

# Debug info (§6.2 item 10): the produced binary MUST carry DWARF < 4 (Mayhem triage
# cannot read DWARF >= 4). rustc nightly defaults to DWARF-5, so pin -Zdwarf-version=3
# for Rust code; the libfuzzer-sys cc shim is compiled by clang (DWARF-5 default), so
# pin its DWARF via CFLAGS/CXXFLAGS. $RUST_DEBUG_FLAGS threads any extra base pins.
export RUSTFLAGS="${RUSTFLAGS:-} ${RUST_DEBUG_FLAGS:-} --cfg fuzzing --cfg jsonrpc_fuzz ${RUST_SAN} -Zdwarf-version=3 -Cdebuginfo=1 -Cforce-frame-pointers"
export CFLAGS="${CFLAGS:-} -gdwarf-3"
export CXXFLAGS="${CXXFLAGS:-} -gdwarf-3"

# The bundled ASan runtime archive that `-Zsanitizer=address` links is precompiled
# with clang (DWARF-5) and ships with full debug info, which would otherwise land a
# DWARF-5 compile unit as the binary's FIRST CU and fail the DWARF < 4 gate. Strip
# debug info from that runtime archive (a toolchain artifact, NOT project code).
# Idempotent: re-running --strip-debug on an already-stripped archive is a no-op,
# so the offline re-run stays clean.
if [ -n "${RUST_SAN}" ]; then
  RT_LIB_DIR="$(rustc +"${RUST_CHANNEL}" --print sysroot)/lib/rustlib/${TRIPLE}/lib"
  for asan in "$RT_LIB_DIR"/librustc-*_rt.asan.a; do
    [ -f "$asan" ] || continue
    if [ -w "$asan" ]; then
      objcopy --strip-debug "$asan" "$asan.stripped" && mv "$asan.stripped" "$asan"
      echo "stripped debug info from bundled ASan runtime: $asan"
    fi
  done
fi

# Our additive fuzz crate. It is its OWN workspace (mayhem/fuzz/Cargo.toml has
# `[workspace] members = ["."]`), so cargo-fuzz writes binaries under
# mayhem/fuzz/target/, NOT the repo-root target/. Discover every target from
# fuzz_targets/.
FUZZ_DIR="mayhem/fuzz"
FUZZ_TARGETS=()
for f in "$FUZZ_DIR"/fuzz_targets/*.rs; do
  FUZZ_TARGETS+=("$(basename "${f%.*}")")
done
[ "${#FUZZ_TARGETS[@]}" -gt 0 ] || { echo "ERROR: no fuzz targets under $FUZZ_DIR/fuzz_targets/" >&2; exit 1; }

echo "=== cargo fuzz build (pinned nightly, ASan via RUSTFLAGS) ==="
echo "RUSTFLAGS=$RUSTFLAGS"
echo "targets: ${FUZZ_TARGETS[*]}"

for t in "${FUZZ_TARGETS[@]}"; do
  echo "--- building fuzz target: $t ---"
  cargo +"${RUST_CHANNEL}" fuzz build --fuzz-dir "$FUZZ_DIR" -O --debug-assertions "$t"
  bin="$SRC/$FUZZ_DIR/target/$TRIPLE/release/$t"
  [ -x "$bin" ] || { echo "ERROR: expected fuzz binary not found at $bin" >&2; exit 1; }
  cp "$bin" "/mayhem/$t"
  echo "built /mayhem/$t"
done

echo "build.sh complete"
