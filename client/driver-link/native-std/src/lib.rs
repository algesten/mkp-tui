//! Blocking-rustls worker for the link driver.
//!
//! Architecture (no tokio):
//! ```text
//!   manager thread (always alive)
//!   ────────────────────────────
//!   outer loop:   drain cmd_rx for ConnectClient / ConnectPair
//!   on Connect:   handshake, spawn reader, enter inner loop
//!   inner loop:   drain cmd_rx (Send / Disconnect / ConfirmPair …)
//!                 watch close_rx for reader-signaled shutdown
//!   on teardown:  shutdown socket, join reader, emit Closed, back to outer
//!
//!   reader thread (per active connection)
//!   ─────────────────────────────────────
//!   owns cloned TcpStream + shared Arc<Mutex<ClientConnection>>
//!   blocking read → decrypt under mutex → decode frames → emit events
//!   exits on socket EOF/error; signals manager via close_tx
//! ```
//!
//! Concurrency notes:
//! - The Mutex on `ClientConnection` also serialises writes to the
//!   socket fd (both threads hold it when calling `write_tls`), so
//!   there is no interleave of half-TLS-records on the wire.
//! - The reader does blocking socket reads *without* holding the
//!   Mutex, so writes are not blocked on a quiet peer.

mod tls;

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use log::{debug, error, info, warn};
use rustls::pki_types::ServerName;
use rustls::ClientConnection;

use mkpclient_core::Notifier;
use mkpclient_driver_link_core::{LinkCmd, LinkDriver, LinkEvent, LinkKind, Trace};
use mkproto::{
    codec, compute_pairing_code, PairClientMsg, PairServerMsg, Request, Response, ServerMsg,
};

/// Internal manager-thread event. Wraps the public `LinkCmd` plus
/// synthetic variants the reader thread posts when its socket dies.
/// A small adapter thread forwards `LinkCmd`s from the runtime onto
/// this channel so the manager only has one thing to block on.
enum ManagerEvent {
    Cmd(LinkCmd),
    ReaderClosed(Option<String>),
}

pub struct LinkNative {
    _marker: (),
}

pub fn spawn(trace: Arc<dyn Trace>, notify: Notifier) -> (LinkDriver, LinkNative) {
    tls::ensure_crypto_provider();

    // Runtime-facing: a plain `Sender<LinkCmd>`.
    let (cmd_tx, cmd_rx) = mpsc::channel::<LinkCmd>();
    // Internal merged stream the manager blocks on.
    let (mgr_tx, mgr_rx) = mpsc::channel::<ManagerEvent>();
    let (event_tx, event_rx) = mpsc::channel::<LinkEvent>();

    // Adapter: forward LinkCmd → ManagerEvent::Cmd. Dies when the
    // runtime drops the sync driver.
    let adapter_tx = mgr_tx.clone();
    thread::Builder::new()
        .name("mkp-link-cmds".into())
        .spawn(move || {
            while let Ok(cmd) = cmd_rx.recv() {
                if adapter_tx.send(ManagerEvent::Cmd(cmd)).is_err() {
                    return;
                }
            }
        })
        .expect("spawning link cmd adapter should succeed");

    thread::Builder::new()
        .name("mkp-link".into())
        .spawn(move || manager_loop(mgr_rx, mgr_tx, event_tx, notify))
        .expect("spawning link manager thread should succeed");

    let driver = LinkDriver::new(cmd_tx, event_rx, trace);
    (driver, LinkNative { _marker: () })
}

fn manager_loop(
    mgr_rx: Receiver<ManagerEvent>,
    mgr_tx: Sender<ManagerEvent>,
    event_tx: Sender<LinkEvent>,
    notify: Notifier,
) {
    while let Ok(event) = mgr_rx.recv() {
        let ManagerEvent::Cmd(cmd) = event else {
            // ReaderClosed while idle — reader should only exist
            // while connected, so this is a stale signal. Drop.
            continue;
        };
        match cmd {
            LinkCmd::ConnectClient {
                addr,
                server_cert_pem,
                client_cert_pem,
                client_key_pem,
                fingerprint: _,
            } => match connect_client(&addr, &server_cert_pem, &client_cert_pem, &client_key_pem) {
                Ok((conn, tcp)) => {
                    run_client_connected(conn, tcp, &mgr_rx, &mgr_tx, &event_tx, &notify);
                }
                Err(e) => emit_closed(&event_tx, &notify, Some(e)),
            },
            LinkCmd::ConnectPair {
                addr,
                server_name: _,
            } => match begin_pairing(&addr) {
                Ok(pairing) => {
                    run_pairing(pairing, &mgr_rx, &event_tx, &notify);
                }
                Err(e) => emit_closed(&event_tx, &notify, Some(e)),
            },
            LinkCmd::ProbeFingerprint { addr } => {
                let result = do_probe(&addr);
                let _ = event_tx.send(LinkEvent::ProbeResult { addr, result });
                notify.notify();
            }
            LinkCmd::Send { .. }
            | LinkCmd::Disconnect
            | LinkCmd::ConfirmPair
            | LinkCmd::RejectPair => {
                debug!("link: ignoring {:?} while idle", cmd_kind(&cmd));
            }
        }
    }
}

// ─── client-mode connection ─────────────────────────────────────────

fn connect_client(
    addr: &str,
    server_cert_pem: &str,
    client_cert_pem: &str,
    client_key_pem: &str,
) -> Result<(ClientConnection, TcpStream), String> {
    info!("link: connect_client addr={addr}");
    let config = tls::authenticated_config(server_cert_pem, client_cert_pem, client_key_pem)?;
    let server_name = ServerName::try_from("mkplay").expect("valid server name");
    let mut conn = ClientConnection::new(config, server_name)
        .map_err(|e| format!("rustls ClientConnection: {e}"))?;

    let mut tcp = TcpStream::connect(addr).map_err(|e| format!("TCP connect: {e}"))?;
    tcp.set_nodelay(true).ok();
    info!("link: TCP connected to {addr}, starting TLS handshake");

    // Drive the handshake on this thread, blocking. `complete_io`
    // loops internally calling `read_tls` / `write_tls` until
    // `is_handshaking()` returns false.
    conn.complete_io(&mut tcp)
        .map_err(|e| format!("TLS handshake: {e}"))?;
    info!("link: TLS handshake complete");

    Ok((conn, tcp))
}

fn run_client_connected(
    conn: ClientConnection,
    tcp: TcpStream,
    mgr_rx: &Receiver<ManagerEvent>,
    mgr_tx: &Sender<ManagerEvent>,
    event_tx: &Sender<LinkEvent>,
    notify: &Notifier,
) {
    let conn = Arc::new(Mutex::new(conn));

    let mut tcp_write = tcp;
    let tcp_read = match tcp_write.try_clone() {
        Ok(t) => t,
        Err(e) => {
            emit_closed(event_tx, notify, Some(format!("TcpStream::try_clone: {e}")));
            return;
        }
    };

    // Reader blocks on `tcp_read.read()` forever. When we want to
    // tear down we call `tcp_write.shutdown(Shutdown::Both)`: that
    // wakes the reader's blocking syscall on all platforms we
    // target, so we don't need read timeouts just to poll shutdown
    // flags.
    let reader_handle = spawn_reader(
        tcp_read,
        conn.clone(),
        event_tx.clone(),
        notify.clone(),
        mgr_tx.clone(),
        Mode::Client,
    );

    let _ = event_tx.send(LinkEvent::Connected {
        kind: LinkKind::Client,
    });
    notify.notify();

    let mut forced_error: Option<String> = None;
    // Block on the merged event channel — no timers, no polling.
    while let Ok(event) = mgr_rx.recv() {
        match event {
            ManagerEvent::Cmd(LinkCmd::Send { seq, task_id, msg }) => {
                let req = Request { seq, task_id, msg };
                match codec::encode_frame(&req) {
                    Ok(frame) => {
                        if let Err(e) = write_plaintext(&conn, &mut tcp_write, &frame) {
                            forced_error = Some(format!("send: {e}"));
                            break;
                        }
                    }
                    Err(e) => warn!("link: encode Request seq={seq} failed: {e}"),
                }
            }
            ManagerEvent::Cmd(LinkCmd::Disconnect) => break,
            ManagerEvent::Cmd(_) => {
                // ignore ConnectClient / ConnectPair / Probe /
                // Confirm / Reject while a client link is active.
            }
            ManagerEvent::ReaderClosed(err) => {
                forced_error = err;
                break;
            }
        }
    }

    let _ = tcp_write.shutdown(Shutdown::Both);
    let _ = reader_handle.join();
    emit_closed(event_tx, notify, forced_error);
}

// ─── pairing-mode connection ────────────────────────────────────────

struct PairingCtx {
    conn: ClientConnection,
    tcp: TcpStream,
    server_cert_pem: String,
    client_cert_pem: String,
    client_key_pem: String,
    fingerprint: String,
    code: String,
}

fn begin_pairing(addr: &str) -> Result<PairingCtx, String> {
    // Fresh key + CSR.
    use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
    let key_pair =
        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|e| format!("keygen: {e}"))?;
    let mut params =
        CertificateParams::new(Vec::<String>::new()).map_err(|e| format!("cert params: {e}"))?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Make Play Client");
    let csr = params
        .serialize_request(&key_pair)
        .map_err(|e| format!("CSR: {e}"))?;
    let csr_pem = csr.pem().map_err(|e| format!("CSR PEM: {e}"))?;
    let client_key_pem = key_pair.serialize_pem();

    // TLS over TOFU: capture the server cert DER in the verifier.
    let (config, captured_cert) = tls::pairing_config();
    let server_name = ServerName::try_from("mkplay").expect("valid server name");
    let mut conn = ClientConnection::new(config, server_name)
        .map_err(|e| format!("rustls ClientConnection: {e}"))?;

    let mut tcp = TcpStream::connect(addr).map_err(|e| format!("TCP connect: {e}"))?;
    tcp.set_nodelay(true).ok();

    conn.complete_io(&mut tcp)
        .map_err(|e| format!("TLS handshake: {e}"))?;

    let server_cert_der = captured_cert
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .ok_or("pairing: server cert not captured")?;
    let fingerprint = tls::cert_fingerprint(&server_cert_der);
    let server_cert_pem = tls::der_to_cert_pem(&server_cert_der);

    // Send PairRequest.
    let req = PairClientMsg::PairRequest { csr_pem };
    let frame = codec::encode_frame(&req).map_err(|e| format!("encode PairRequest: {e}"))?;
    conn.writer()
        .write_all(&frame)
        .map_err(|e| format!("write PairRequest plaintext: {e}"))?;
    while conn.wants_write() {
        conn.write_tls(&mut tcp)
            .map_err(|e| format!("write PairRequest TLS: {e}"))?;
    }

    // Read PairResponse.
    let pair_reply = read_one_pair_frame(&mut conn, &mut tcp)?;
    let client_cert_pem = match pair_reply {
        PairServerMsg::PairResponse { client_cert_pem } => client_cert_pem,
        PairServerMsg::PairError { message } => {
            return Err(format!("PAIR_ERR:{message}"));
        }
    };

    let ekm = tls::export_keying_material(&conn);
    let client_cert_der = tls::cert_pem_to_der(&client_cert_pem);
    let code = compute_pairing_code(&ekm, &client_cert_der);

    Ok(PairingCtx {
        conn,
        tcp,
        server_cert_pem,
        client_cert_pem,
        client_key_pem,
        fingerprint,
        code,
    })
}

fn run_pairing(
    mut ctx: PairingCtx,
    mgr_rx: &Receiver<ManagerEvent>,
    event_tx: &Sender<LinkEvent>,
    notify: &Notifier,
) {
    let _ = event_tx.send(LinkEvent::Connected {
        kind: LinkKind::Pairing,
    });
    let _ = event_tx.send(LinkEvent::PairingReady {
        server_cert_pem: ctx.server_cert_pem.clone(),
        client_cert_pem: ctx.client_cert_pem.clone(),
        client_key_pem: ctx.client_key_pem.clone(),
        fingerprint: ctx.fingerprint.clone(),
        code: ctx.code.clone(),
    });
    notify.notify();

    // Wait synchronously for the user to push ConfirmPair / RejectPair
    // or Disconnect. No reader thread here — the server only sends
    // one frame (the PairResponse we already consumed) and will
    // close after our Confirm/Reject, so a blocking recv is fine.
    let mut forced_error: Option<String> = None;
    loop {
        match mgr_rx.recv() {
            Ok(ManagerEvent::Cmd(LinkCmd::ConfirmPair)) => {
                if let Err(e) = send_pair_terminal(&mut ctx, &PairClientMsg::PairConfirm) {
                    forced_error = Some(e);
                }
                break;
            }
            Ok(ManagerEvent::Cmd(LinkCmd::RejectPair)) => {
                let _ = send_pair_terminal(&mut ctx, &PairClientMsg::PairReject);
                break;
            }
            Ok(ManagerEvent::Cmd(LinkCmd::Disconnect)) => break,
            Ok(ManagerEvent::Cmd(_)) | Ok(ManagerEvent::ReaderClosed(_)) => {
                // Ignore Send / Connect* / stray reader closes during pairing.
            }
            Err(_) => {
                forced_error = Some("runtime dropped cmd sender".into());
                break;
            }
        }
    }

    let _ = ctx.tcp.shutdown(Shutdown::Both);
    emit_closed(event_tx, notify, forced_error);
}

fn send_pair_terminal(ctx: &mut PairingCtx, msg: &PairClientMsg) -> Result<(), String> {
    let frame = codec::encode_frame(msg).map_err(|e| format!("encode pair terminal: {e}"))?;
    ctx.conn
        .writer()
        .write_all(&frame)
        .map_err(|e| format!("write pair terminal plaintext: {e}"))?;
    while ctx.conn.wants_write() {
        ctx.conn
            .write_tls(&mut ctx.tcp)
            .map_err(|e| format!("write pair terminal TLS: {e}"))?;
    }
    Ok(())
}

fn read_one_pair_frame(
    conn: &mut ClientConnection,
    tcp: &mut TcpStream,
) -> Result<PairServerMsg, String> {
    let mut plaintext = Vec::new();
    let mut scratch = [0u8; 4096];

    loop {
        if let Some((msg, used)) = codec::try_decode::<PairServerMsg>(&plaintext)
            .map_err(|e| format!("decode PairServerMsg: {e}"))?
        {
            plaintext.drain(..used);
            return Ok(msg);
        }

        // Pull ciphertext.
        let n = conn.read_tls(tcp).map_err(|e| format!("read_tls: {e}"))?;
        if n == 0 {
            return Err("connection closed waiting for PairResponse".into());
        }
        conn.process_new_packets()
            .map_err(|e| format!("process_new_packets: {e}"))?;

        // Drain decrypted plaintext.
        loop {
            match conn.reader().read(&mut scratch) {
                Ok(0) => break,
                Ok(m) => plaintext.extend_from_slice(&scratch[..m]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(format!("TLS reader: {e}")),
            }
        }
    }
}

// ─── reader thread ──────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Mode {
    Client,
}

fn spawn_reader(
    mut tcp_read: TcpStream,
    conn: Arc<Mutex<ClientConnection>>,
    event_tx: Sender<LinkEvent>,
    notify: Notifier,
    close_tx: Sender<ManagerEvent>,
    mode: Mode,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("mkp-link-reader".into())
        .spawn(move || {
            let mut plaintext: Vec<u8> = Vec::new();
            let mut scratch = [0u8; 4096];

            let close = |err: Option<String>, tx: &Sender<ManagerEvent>| {
                let _ = tx.send(ManagerEvent::ReaderClosed(err));
            };

            loop {
                if let Err(e) = emit_pending_frames(&mut plaintext, &event_tx, &notify, mode) {
                    close(Some(e), &close_tx);
                    return;
                }

                // Pure blocking read: unblocks on peer EOF or on
                // `shutdown(Shutdown::Both)` from the manager. No
                // timeouts, no polling.
                let ciphertext: Vec<u8> = {
                    let mut buf = [0u8; 8 * 1024];
                    match tcp_read.read(&mut buf) {
                        Ok(0) => {
                            close(None, &close_tx);
                            return;
                        }
                        Ok(n) => buf[..n].to_vec(),
                        Err(e) => {
                            close(Some(format!("socket read: {e}")), &close_tx);
                            return;
                        }
                    }
                };

                let decrypt_err = {
                    let mut c = match conn.lock() {
                        Ok(g) => g,
                        Err(_) => {
                            close(Some("poisoned conn mutex".into()), &close_tx);
                            return;
                        }
                    };

                    // Feed ciphertext into rustls, draining the
                    // internal buffer as we go. `read_tls(&mut &[u8])`
                    // advances the slice reference, and rustls's
                    // record-layer buffer can stop consuming once
                    // it's full — if we don't loop and drain, the
                    // tail of our local buffer is silently dropped
                    // and the next socket read arrives misaligned
                    // (MAC failure on the next record).
                    let mut src: &[u8] = &ciphertext;
                    let mut err: Option<String> = None;
                    while !src.is_empty() {
                        match c.read_tls(&mut src) {
                            Ok(0) => {
                                // rustls refused to take more bytes
                                // even though the internal buffer
                                // should be drainable; break to
                                // avoid spinning.
                                break;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                err = Some(format!("read_tls: {e}"));
                                break;
                            }
                        }
                        if let Err(e) = c.process_new_packets() {
                            err = Some(format!("process_new_packets: {e}"));
                            break;
                        }
                        loop {
                            match c.reader().read(&mut scratch) {
                                Ok(0) => break,
                                Ok(m) => plaintext.extend_from_slice(&scratch[..m]),
                                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                                Err(e) => {
                                    err = Some(format!("TLS reader: {e}"));
                                    break;
                                }
                            }
                        }
                        if err.is_some() {
                            break;
                        }
                    }
                    err
                };
                if let Some(e) = decrypt_err {
                    close(Some(e), &close_tx);
                    return;
                }
            }
        })
        .expect("spawning link reader thread should succeed")
}

fn emit_pending_frames(
    buf: &mut Vec<u8>,
    event_tx: &Sender<LinkEvent>,
    notify: &Notifier,
    mode: Mode,
) -> Result<(), String> {
    loop {
        match mode {
            Mode::Client => {
                match codec::try_decode::<Response>(buf)
                    .map_err(|e| format!("decode Response: {e}"))?
                {
                    Some((resp, used)) => {
                        buf.drain(..used);
                        if event_tx.send(LinkEvent::Frame(Box::new(resp))).is_err() {
                            return Err("runtime dropped event receiver".into());
                        }
                        notify.notify();
                    }
                    None => return Ok(()),
                }
            }
        }
    }
}

// ─── small helpers ──────────────────────────────────────────────────

fn write_plaintext(
    conn: &Arc<Mutex<ClientConnection>>,
    tcp: &mut TcpStream,
    frame: &[u8],
) -> Result<(), String> {
    let mut c = conn.lock().map_err(|_| "poisoned conn mutex".to_string())?;
    c.writer()
        .write_all(frame)
        .map_err(|e| format!("rustls writer: {e}"))?;
    while c.wants_write() {
        c.write_tls(tcp).map_err(|e| format!("write_tls: {e}"))?;
    }
    Ok(())
}

fn emit_closed(event_tx: &Sender<LinkEvent>, notify: &Notifier, error: Option<String>) {
    if let Some(ref e) = error {
        error!("link: closing with error: {e}");
    } else {
        debug!("link: closing cleanly");
    }
    let _ = event_tx.send(LinkEvent::Closed { error });
    notify.notify();
}

fn cmd_kind(cmd: &LinkCmd) -> &'static str {
    match cmd {
        LinkCmd::ConnectClient { .. } => "ConnectClient",
        LinkCmd::ConnectPair { .. } => "ConnectPair",
        LinkCmd::ProbeFingerprint { .. } => "ProbeFingerprint",
        LinkCmd::Disconnect => "Disconnect",
        LinkCmd::Send { .. } => "Send",
        LinkCmd::ConfirmPair => "ConfirmPair",
        LinkCmd::RejectPair => "RejectPair",
    }
}

fn do_probe(addr: &str) -> Result<String, String> {
    let (config, captured) = tls::probe_config();
    let server_name = ServerName::try_from("mkplay").expect("valid server name");
    let mut conn = ClientConnection::new(config, server_name)
        .map_err(|e| format!("rustls ClientConnection: {e}"))?;
    let mut tcp = TcpStream::connect(addr).map_err(|e| format!("TCP connect: {e}"))?;
    tcp.set_nodelay(true).ok();
    // Setting a read timeout keeps a hung server from wedging the
    // manager thread forever; 5 s is plenty for a LAN handshake.
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
    conn.complete_io(&mut tcp)
        .map_err(|e| format!("TLS handshake: {e}"))?;

    let cert_der = captured
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .ok_or_else(|| "probe: server cert not captured".to_string())?;
    // Best-effort graceful close so the server logs don't see an abort.
    conn.send_close_notify();
    while conn.wants_write() {
        if conn.write_tls(&mut tcp).is_err() {
            break;
        }
    }
    let _ = tcp.shutdown(Shutdown::Both);
    Ok(tls::cert_fingerprint(&cert_der))
}

// Silence unused-import warning — ServerMsg re-exported for downstream
// crates constructing Response values in tests.
#[allow(dead_code)]
fn _force_servermsg_link() -> Option<ServerMsg> {
    None
}
