# mkp-tui

The open-source half of [Make Play](https://github.com/algesten/mkp): the wire
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

## Build and run

```bash
cargo build --release -p mkpclient-tui   # produces target/release/mkp
cargo run -p mkpclient-tui               # discovers a server via mDNS
```

Builds on macOS and Linux. Rust via [rustup](https://rustup.rs/).

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
