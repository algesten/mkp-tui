//! TLS mock server speaking the mkp wire protocol.
//!
//! Spawns a thread that accepts a single TLS connection, reads
//! `Request` frames, and replies with frames pushed onto a script
//! before the test starts (or via a closure that maps requests to
//! responses). Closes when the test drops the handle.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use mkproto::{ClientMsg, Request, Response, ServerMsg};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;

use super::certs::TestCerts;

/// Sequence of server messages to send back when a request is
/// received. seq=0 means broadcast (no correlation), otherwise the
/// mock answers with the request's seq.
#[allow(dead_code)] // variants picked by individual scenario scripts
pub enum ScriptStep {
    /// Reply directly to a matched `ClientMsg` with the given
    /// response message (sent on the request's seq).
    Reply(ServerMsg),
    /// Broadcast (seq=0). Sent before processing the next request.
    Broadcast(ServerMsg),
    /// Broadcast carrying a task_id (used for SearchMore streaming).
    BroadcastWithTask { task_id: u64, msg: ServerMsg },
}

pub type Script = Box<dyn Fn(&ClientMsg) -> Vec<ScriptStep> + Send + Sync>;

pub struct MockServer {
    pub addr: SocketAddr,
    pub certs: Arc<TestCerts>,
    /// Every ClientMsg the server has decoded — useful for
    /// assertions in scenarios that care about traffic.
    #[allow(dead_code)]
    received: Arc<Mutex<Vec<ClientMsg>>>,
    _handle: JoinHandle<()>,
}

impl MockServer {
    pub fn start(certs: TestCerts, script: Script) -> Self {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        let cert_der = CertificateDer::from(certs.server_cert_der.clone());
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certs.server_key_der.clone()));
        let mut cfg = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .expect("server cert/key");
        cfg.alpn_protocols = vec![b"mkp-client".to_vec(), b"mkp-pair".to_vec()];
        let cfg = Arc::new(cfg);

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let received = Arc::new(Mutex::new(Vec::<ClientMsg>::new()));
        let received_for_thread = received.clone();
        let certs = Arc::new(certs);

        let handle = std::thread::spawn(move || {
            let cfg = cfg;
            // Accept a single connection; loop in case the runtime
            // re-connects, but each connection is sequential.
            while let Ok((tcp, _peer)) = listener.accept() {
                let conn = match rustls::ServerConnection::new(cfg.clone()) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let mut tls = rustls::StreamOwned::new(conn, tcp);
                handle_connection(&mut tls, &script, &received_for_thread);
            }
        });

        MockServer {
            addr,
            certs,
            received,
            _handle: handle,
        }
    }

    #[allow(dead_code)]
    pub fn received(&self) -> Vec<ClientMsg> {
        self.received.lock().unwrap().clone()
    }
}

fn handle_connection(
    tls: &mut rustls::StreamOwned<rustls::ServerConnection, std::net::TcpStream>,
    script: &Script,
    received: &Arc<Mutex<Vec<ClientMsg>>>,
) {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    loop {
        match tls.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                while let Some(total) = mkproto::codec::frame_len(&buf) {
                    let payload = &buf[4..total];
                    let req: Request = match mkproto::decode_frame(payload) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("mock: decode err {e}");
                            return;
                        }
                    };
                    let drained = buf.drain(..total).len();
                    let _ = drained;
                    received.lock().unwrap().push(req.msg.clone());
                    let steps = script(&req.msg);
                    for step in steps {
                        let resp = match step {
                            ScriptStep::Reply(msg) => Response {
                                seq: req.seq,
                                task_id: req.task_id,
                                msg,
                            },
                            ScriptStep::Broadcast(msg) => Response {
                                seq: 0,
                                task_id: None,
                                msg,
                            },
                            ScriptStep::BroadcastWithTask { task_id, msg } => Response {
                                seq: 0,
                                task_id: Some(task_id),
                                msg,
                            },
                        };
                        let frame = match mkproto::encode_frame(&resp) {
                            Ok(f) => f,
                            Err(_) => return,
                        };
                        if tls.write_all(&frame).is_err() {
                            return;
                        }
                    }
                    let _ = tls.flush();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => break,
        }
    }
}
