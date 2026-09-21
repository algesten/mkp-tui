# mkp-tui

The open-source half of [Make Play](https://makeplayapp.com/): the wire
protocol and the cross-platform client that speaks it, including the ratatui
terminal UI.

The Apple-side server — a macOS menu bar app wrapping MusicKit, plus the iOS
app — lives in a separate closed repository and consumes this one as a git
submodule.

```
                       ┌───────────────────────────┐
                       │Menubar App  (closed)      │
                       │                           │
                       │┌─────────────────────────┐│
                       ││Swift                    ││
                       ││  ┌─────────────────────┐││
                       ││  │   Apple MusicKit    │││
                       ││  └─────────────────────┘││
                       │├─────────────────────────┤│
                       ││           FFI           ││
                       │├─────────────────────────┤│
                       ││       Rust Server       ││
                       │└─────────────────────────┘│
                       └───────────────────────────┘
                                     │
             ┌───────────────────────┴──────────────────────┐
             │                 mDNS discovery               │
             │              TLS + TCP over LAN              │
             │                                              │
┌─────────────────────────┐                    ┌─────────────────────────┐
│TUI Client   (this repo) │                    │TUI Client   (this repo) │
│┌───────────────────────┐│                    │┌───────────────────────┐│
││      Rust Client      ││                    ││      Rust Client      ││
│├───────────────────────┤│                    │├───────────────────────┤│
││        Ratatui        ││                    ││        Ratatui        ││
│└───────────────────────┘│                    │└───────────────────────┘│
└─────────────────────────┘                    └─────────────────────────┘
```

## Install

The client is `mkp`. It needs a Make Play server running on a Mac on the
same network — it discovers one over mDNS and pairs with it.

**Homebrew** (macOS and Linux):

```bash
brew install algesten/make-play/mkp
```

The fully qualified name matters. Homebrew does not load formulae from
third-party taps until they are trusted, and installing by full name
trusts this one formula — nothing else the tap might ever contain. The
longer route below is equivalent but grants trust to the whole tap:

```bash
brew tap algesten/make-play
brew trust algesten/make-play
brew install mkp
```

Homebrew predating tap trust has no `trust` command and answers
`Error: Unknown command: brew trust`; that version does not need it.

**Nix**:

```bash
nix run github:algesten/mkp-tui#mkp
```

**From source**, any platform with a Rust toolchain:

```bash
cargo install --locked --git https://github.com/algesten/mkp-tui
```

Cargo installs from `main`, which stays stable and backwards compatible but
can include changes ahead of the latest promoted release. Homebrew and the
website downloads follow the latest promoted release. GitHub prereleases can
be replaced while being tested; promoted versions are frozen.

**Prebuilt Linux binaries** — statically linked x86_64 and aarch64 builds
are attached to every [release](https://github.com/algesten/mkp-tui/releases/latest).
They carry no runtime dependencies, so they run on any distribution
regardless of its glibc version.

## Build from a checkout

```bash
cargo build --release -p mkpclient-tui   # produces target/release/mkp
cargo run -p mkpclient-tui               # discovers a server via mDNS
```

Builds on macOS and Linux. Rust via [rustup](https://rustup.rs/).

`mkp --version` reports the release version when one was supplied at build
time, and `<tag>-<n>-g<sha>` for a build off an untagged commit.

## Startup diagnostics

From this checkout, capture startup while keeping the TUI on the terminal:

```bash
RUST_LOG=trace cargo run --release -p mkpclient-tui 2>/tmp/mkp.log
```

Use `RUST_LOG=mkp_startup=trace` to capture only client timing events.
When running from the parent Make Play repository, add
`--manifest-path tui/Cargo.toml --target-dir target` to the cargo command.
Logs go to **stderr**; redirecting stdout captures terminal output instead.

Each line includes wall-clock time, monotonic milliseconds since process
startup, and the thread name. The `mkp_startup` target records worker setup,
credential/persistence I/O, discovery, certificate probes, TCP/TLS, request
queueing, frame encoding/writes/reads/decoding, ingestion, lifecycle stages,
view restoration, rendering, terminal drawing, and wake/deadline waits.
`seq` correlates requests and replies; `task` correlates streamed follow-ups.
`request_round_trip` measures driver dispatch to event drain (including local
queueing), not pure network latency. `view_drawn` reports changing readiness
and row counts after a successful draw. A restored view is not necessarily
fully loaded: inspect subsequent chunks, task completion, and rendered counts.

Compare `socket_write_done` to `frame_decoded` to locate server/network waits,
then `ingest_frame_done` and `view_drawn` to locate client-side delays. Durations
are in microseconds. Nested durations include tracing overhead; full trace
logging affects the measurements. Credentials and full song payloads are not
logged by these timing events. Logs continue after startup to capture late
chunks and reconnects; quit once the view and playback state have settled.

### Repeatable startup benchmark

On macOS or Linux with Python 3.9+, build the release binary, then run:

```bash
cargo build --release -p mkpclient-tui
python3 scripts/benchmark-startup.py \
  --binary target/release/mkp --server SERVER_NAME \
  --runs 20 --warmup 1 --timeout 30 \
  --fixture "server build/version; saved view; warm server" \
  --output /tmp/mkp-startup-benchmark
```

Use the exact paired name from the server picker. The output directory must
be new. The harness launches the actual TUI in a 140×40 pseudo-terminal,
continuously drains its output, and enables only `RUST_LOG=mkp_startup=trace`.
It snapshots your configuration into a private temporary directory and copies
that snapshot for every run, selecting the requested server there. It sends no
keyboard or playback commands and does not change your normal configuration.
`--config`, `--rows`, and `--cols` override the fixture inputs.

`report.json` contains every measured run and warmup, failures/timeouts, missing
milestones, client version/profile/OS/architecture, binary and saved-view hashes,
fixture dimensions, and median/p95 timings. `summary.txt` is the readable summary;
each run also has a TRACE log. Percentiles use nearest rank on successful complete
runs only, with their sample count and failed/timed-out runs explicitly reported.
Partial timings remain in the JSON. Any failed warmup, failed/timed-out measured
run, or interruption makes the command exit nonzero. Keep fixture dimensions,
server build, and logging settings consistent between comparisons; record server
TRACE settings in `--fixture`. A small sample is a smoke check, not a reliable p95.

The baseline is **authenticated TLS completion**, excluding discovery and the
preliminary certificate probe. Process-to-TLS time is reported separately.
All five milestones are observed after successful terminal draws:

- `playback_queue`: a playback snapshot (including stopped/no song) and a queue
  snapshot with its announced rows, including an explicitly empty queue.
- `sidebar`: the playlist list has arrived, including an empty library.
- `visible_view`: navigation is restored and the current viewport contains loaded
  rows; an explicitly loaded empty playlist/result is ready. For playlist views,
  pending slots in the viewport prevent readiness.
- `complete_view`: the saved view's data and its correlated content stream are
  complete; merely restoring navigation or receiving `ListBegin` is insufficient.
- `background_complete`: all the above plus completion of this client's startup
  streams, including playlist counts. Other peers' task broadcasts are ignored.

Readiness is based on what the server has announced; it cannot certify upstream
freshness or predict later unsolicited updates. Request/task failures and
reconnects fail a sample even if the UI falls back to an empty view. Terminal draw
completion includes model construction and buffer output, not a physical display's
refresh. Server/request/render timing events explain the interval between milestones.

Scenarios are separate reports. `--scenario warm` optionally warms once before
measurement. `--scenario restarted` and `--scenario uncached` **require** an
executable `--prepare /path/to/script`, invoked before every warmup and measured
sample, with `--prepare-timeout` (default 60 seconds). The executable receives no
arguments and must synchronously establish the named fixture, e.g. restart and
wait for the server, or reset the intended uncached server fixture. A preparation
failure is recorded and that client launch is skipped. The harness does not assume
that repeated connections remain cold, and does not itself restart your server or
clear its caches. Describe the preparation in `--fixture` and use a matching saved
view in `--config` for uncached-view tests.

Warm cached targets: p95 within **50 ms after TLS** for playback, queue, sidebar,
and visible saved-view rows; within **100 ms** for a representative cached
206-track complete view. These are measurement targets, not claims that an
uncached upstream fetch or an arbitrary-size library can meet them.

Normal runs need no diagnostic flags: unset `RUST_LOG` (or choose `info`) to omit
these TRACE events. The benchmark always enables them and records that setting;
trace output adds overhead, so compare like with like.

## Layout

- **proto/** — `mkproto`: shared protocol types and the length-prefixed
  MessagePack codec (`[4 bytes BE length][msgpack payload]`). The `mdns`
  feature (default on) adds discovery and advertising; client crates turn it
  off to keep tokio out of their dependency tree.
- **client/core/** — `mkpclient-core`: the primitives every state and driver
  crate builds on.
- **client/state-\*/** — one crate per unit of client state, composed with
  [`drv`](https://crates.io/crates/drv) memoized queries. Platform-neutral and
  free of I/O.
- **client/driver-\*/** — effects. Each driver has a `core` crate defining the
  port and one or more native implementations (`native-std`, `native-fs`)
  supplying it. Apple-specific implementations live in the closed repo and
  depend on the `core` crates here.
- **client/runtime/** — assembles the state crates and driver ports into a
  tickable runtime.
- **client/runtime-desktop/** — desktop wiring: mDNS via `opslag`, rustls TLS,
  filesystem credentials and persistence.
- **client/driver-ui-paint-tui/** — terminal paint driver.
- **client/tui/** — the `mkp` binary: ratatui rendering, navigation, keybindings.

## Client pairing and TLS

All client–server communication is TLS encrypted. Clients authenticate with
mutual TLS using a client certificate issued during a one-time pairing flow.

ALPN identifiers distinguish connection types:

- `mkp-pair` — pairing connection, no client certificate required
- `mkp-client` — authenticated connection, client certificate required

### Pairing flow

1. The client discovers the server via mDNS and connects with ALPN `mkp-pair`.
   If the server has pairing toggled off, it rejects the handshake at the ALPN
   stage.
2. The client generates an EC keypair locally and builds a CSR, then sends
   `PairRequest { csr }`.
3. The server signs the CSR with its CA key, producing a client certificate.
4. Both sides independently compute a 6-digit verification code:
   `truncate_to_6_digits(HMAC-SHA256(export_keying_material, client_cert_bytes))`
   — `export_keying_material` is the TLS channel binding, unique per session;
   `client_cert_bytes` is unique per pairing attempt.
5. The server displays its code and sends `PairResponse { client_cert }`.
6. The client prompts for the code and compares it against the one it derived.
   On a match it stores the pinned server certificate fingerprint, the client
   certificate and the private key, then sends `PairConfirm`. On a mismatch it
   discards everything, sends `PairReject`, and warns about a possible MITM.

### MITM protection

A man-in-the-middle terminates TLS on both sides, creating two separate
sessions with different `export_keying_material` values. The code it re-derives
cannot match the one shown on the real server, so the user sees the mismatch
and rejects the pairing.

### Subsequent connections

The client connects with ALPN `mkp-client` and presents its stored certificate
(mTLS). It verifies the server certificate fingerprint against the value pinned
at pairing time; the server verifies the client certificate was signed by its
CA.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
