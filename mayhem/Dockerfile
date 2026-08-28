# syntax=docker/dockerfile:1.7
#
# Commit-image Dockerfile — RUST target (cargo / cargo-fuzz). Builds ON TOP of the
# org C/C++ base (ghcr.io/savantenvs/base), adding a pinned Rust nightly + cargo-fuzz
# under /opt/toolchains/rust, then runs mayhem/build.sh and mayhem/test.sh.
#
# WHY a fixed, $HOME-independent toolchain prefix (SPEC §6.2 items 8/9, §6.5):
#   The Rust toolchain and its cargo registry are NOT in the base. We install them at
#   /opt/toolchains/rust and pin CARGO_HOME/RUSTUP_HOME there as ENV so the cargo
#   registry the FIRST (online) build populates is found again — by the SAME absolute
#   CARGO_HOME — when the air-gapped PATCH tier re-runs build.sh with NO network.
#
# Build (from the repo root, on the `mayhem` branch):
#   DOCKER_BUILDKIT=1 docker build -f mayhem/Dockerfile \
#       --build-arg COMMIT=$(git rev-parse HEAD) -t rust-jsonrpc-commit:test .

FROM ghcr.io/savantenvs/base:20260621

ARG COMMIT
ARG REPO_URL
# SOURCE_DATE_EPOCH stays an ARG (never an ENV — an empty ENV breaks clang). build.sh unsets it when empty.
ARG SOURCE_DATE_EPOCH
# Undefaulted (no '=') so the base ENV default flows through unless --build-arg overrides it.
ARG SANITIZER_FLAGS

# ── Rust toolchain @ a FIXED, lockable, $HOME-independent prefix ───────────────
# nightly is required for -Zsanitizer=address (the OSS-Fuzz Rust ASan path). Pinned by
# DATE for reproducibility; nightly-2025-06-01 is a fleet known-good (2025-01-15 ICEs on
# `log`). rust-jsonrpc's MSRV is 1.63 and its dep tree is pinned by the committed
# lockfiles (root Cargo-recent.lock + mayhem/{fuzz,kat}/Cargo.lock), so this nightly
# builds everything. NOTE: upstream ships no rust-toolchain.toml, but build.sh/test.sh
# still use explicit `+$RUST_CHANNEL` on every cargo invocation as a guard.
ARG RUST_CHANNEL=nightly-2025-06-01
ARG CARGO_FUZZ_VERSION=0.12.0

ENV RUSTUP_HOME=/opt/toolchains/rust/rustup \
    CARGO_HOME=/opt/toolchains/rust/cargo \
    RUST_CHANNEL=${RUST_CHANNEL} \
    PATH=/opt/toolchains/rust/cargo/bin:${PATH}

USER root
RUN install -d /opt/toolchains/rust \
 && curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o /tmp/rustup-init.sh \
 && sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal \
        --default-toolchain "${RUST_CHANNEL}" \
 && rustup component add rust-src --toolchain "${RUST_CHANNEL}" \
 && cargo install cargo-fuzz --version "${CARGO_FUZZ_VERSION}" --locked \
 && rm -f /tmp/rustup-init.sh \
 # The commit build AND the air-gapped PATCH re-run run as `mayhem` (2000): it must read the
 # toolchain and read/refresh the cargo registry cache, so hand the whole prefix to 2000.
 && chown -R 2000:2000 /opt/toolchains/rust

# Source enters from the build CONTEXT (CI checkout): the FULL repo (incl. .git) → /mayhem.
USER mayhem
WORKDIR /mayhem
COPY --chown=2000:2000 . /mayhem

# Populates $CARGO_HOME (online, this first build) and builds the sanitized fuzz targets + KAT probe.
RUN bash mayhem/build.sh

# Run the behavioral oracle at build time (a failing suite fails the build).
RUN if [ -f mayhem/test.sh ]; then bash mayhem/test.sh; else echo "no mayhem/test.sh"; fi

LABEL org.opencontainers.image.source="${REPO_URL}" \
      org.opencontainers.image.revision="${COMMIT}" \
      rlenv.tier="commit" \
      rlenv.source_date_epoch="${SOURCE_DATE_EPOCH}" \
      rlenv.user="mayhem"

USER mayhem
WORKDIR /mayhem
