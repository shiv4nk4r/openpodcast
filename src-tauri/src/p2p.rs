//! Peer-to-peer studio networking via iroh (QUIC + hole punching + public relays).
//! Invite tickets carry an iroh endpoint ticket, studio password, and settings.
//! Same-LAN uses a direct path; cross-network falls back to n0 relays.
//!
//! Protocol (length-prefixed JSON frames on a bi-directional QUIC stream):
//! - Hello { password, name, participant_id }
//! - Admit / Deny / Waiting
//! - PreviewFrame, ClockSync, RecordStart / RecordStop
//! - FileOffer / FileChunk / FileDone
//! - Reaction, Leave, PeerList

use crate::models::{InviteTicket, StudioSettings};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr};
use iroh_tickets::endpoint::EndpointTicket;
use parking_lot::Mutex;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Application-Layer Protocol Negotiation for OpenPodcast studio sessions.
pub const ALPN: &[u8] = b"openpodcast/1";

const ONLINE_TIMEOUT: Duration = Duration::from_secs(25);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerMessage {
    Hello {
        password: String,
        name: String,
        participant_id: String,
    },
    Admit {
        participant_id: String,
    },
    Deny {
        reason: String,
    },
    Waiting {
        participant_id: String,
        name: String,
    },
    PreviewFrame {
        participant_id: String,
        jpeg_b64: Option<String>,
        pcm_b64: Option<String>,
    },
    ClockSync {
        host_unix_ms: i64,
    },
    RecordStart {
        session_id: String,
        started_at_ms: i64,
    },
    RecordStop,
    FileOffer {
        participant_id: String,
        file_name: String,
        size: u64,
        sha256: String,
    },
    FileChunk {
        file_name: String,
        offset: u64,
        data_b64: String,
    },
    FileDone {
        file_name: String,
    },
    Reaction {
        from: String,
        emoji: String,
    },
    Leave {
        participant_id: String,
    },
    PeerList {
        peers: Vec<PeerInfo>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: String,
    pub name: String,
    pub role: String,
    pub admitted: bool,
}

/// Bi-directional peer session over iroh QUIC.
pub struct PeerStream {
    _endpoint: Endpoint,
    _conn: iroh::endpoint::Connection,
    send: iroh::endpoint::SendStream,
    _recv: iroh::endpoint::RecvStream,
}

pub struct StudioHost {
    pub password: String,
    pub ticket: InviteTicket,
    pub peers: Arc<Mutex<HashMap<String, PeerInfo>>>,
    endpoint: Endpoint,
    shutdown: Arc<Mutex<Option<mpsc::Sender<()>>>>,
}

/// Wait until the endpoint has a relay URL (required for cross-network dials).
pub async fn wait_until_dialable(endpoint: &Endpoint) -> Result<(), String> {
    let _ = tokio::time::timeout(ONLINE_TIMEOUT, endpoint.online()).await;
    let deadline = tokio::time::Instant::now() + ONLINE_TIMEOUT;
    loop {
        let addr = endpoint.addr();
        if addr.relay_urls().next().is_some() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(
                "Could not reach an iroh relay. Check that this Mac is online (not captive Wi‑Fi) and try again."
                    .into(),
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

impl StudioHost {
    pub async fn start(studio: StudioSettings, host_name: String) -> Result<Self, String> {
        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| format!("Failed to start studio network: {e}"))?;

        wait_until_dialable(&endpoint).await?;

        let password: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(12)
            .map(char::from)
            .collect();

        let iroh_ticket = EndpointTicket::new(endpoint.addr()).to_string();
        let ticket = InviteTicket {
            version: 2,
            iroh_ticket,
            password: password.clone(),
            studio,
            host_name,
            created_at: Utc::now(),
        };

        let peers = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::channel::<()>(1);
        let peers_c = peers.clone();
        let password_c = password.clone();
        let ep_accept = endpoint.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = rx.recv() => {
                        ep_accept.close().await;
                        break;
                    }
                    incoming = ep_accept.accept() => {
                        let Some(connecting) = incoming else { break };
                        let peers = peers_c.clone();
                        let password = password_c.clone();
                        tokio::spawn(async move {
                            match connecting.await {
                                Ok(conn) => {
                                    let _ = handle_guest(conn, password, peers).await;
                                }
                                Err(e) => {
                                    tracing::warn!("guest connection failed: {e}");
                                }
                            }
                        });
                    }
                }
            }
        });

        Ok(Self {
            password,
            ticket,
            peers,
            endpoint,
            shutdown: Arc::new(Mutex::new(Some(tx))),
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn encode_ticket(&self) -> Result<String, String> {
        let mut ticket = self.ticket.clone();
        // Refresh dialing info (relay URL / direct addrs may have updated).
        ticket.iroh_ticket = EndpointTicket::new(self.endpoint.addr()).to_string();
        encode_ticket(&ticket)
    }

    pub fn update_studio_settings(&mut self, settings: StudioSettings) {
        self.ticket.studio = settings;
    }

    pub fn admit(&self, participant_id: &str) {
        if let Some(p) = self.peers.lock().get_mut(participant_id) {
            p.admitted = true;
        }
    }

    pub fn peer_list(&self) -> Vec<PeerInfo> {
        self.peers.lock().values().cloned().collect()
    }

    pub fn shutdown(&self) {
        if let Some(tx) = self.shutdown.lock().take() {
            let _ = tx.try_send(());
        }
    }
}

async fn handle_guest(
    conn: iroh::endpoint::Connection,
    password: String,
    peers: Arc<Mutex<HashMap<String, PeerInfo>>>,
) -> Result<(), String> {
    let (mut send, mut recv) = conn
        .accept_bi()
        .await
        .map_err(|e| format!("accept stream: {e}"))?;

    let msg = read_frame(&mut recv).await?;
    match msg {
        PeerMessage::Hello {
            password: pw,
            name,
            participant_id,
        } => {
            if pw != password {
                write_frame(
                    &mut send,
                    &PeerMessage::Deny {
                        reason: "Invalid studio password".into(),
                    },
                )
                .await?;
                return Ok(());
            }
            peers.lock().insert(
                participant_id.clone(),
                PeerInfo {
                    id: participant_id.clone(),
                    name: name.clone(),
                    role: "Guest".into(),
                    admitted: false,
                },
            );
            write_frame(
                &mut send,
                &PeerMessage::Waiting {
                    participant_id: participant_id.clone(),
                    name,
                },
            )
            .await?;

            loop {
                let admitted = peers
                    .lock()
                    .get(&participant_id)
                    .map(|p| p.admitted)
                    .unwrap_or(false);
                if admitted {
                    write_frame(
                        &mut send,
                        &PeerMessage::Admit {
                            participant_id: participant_id.clone(),
                        },
                    )
                    .await?;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }

            loop {
                match read_frame(&mut recv).await {
                    Ok(PeerMessage::Leave { .. }) => {
                        peers.lock().remove(&participant_id);
                        break;
                    }
                    Ok(PeerMessage::FileOffer {
                        participant_id,
                        file_name,
                        size: _,
                        sha256: _,
                    }) => {
                        let _ = (participant_id, file_name);
                    }
                    Ok(_) => {}
                    Err(_) => {
                        peers.lock().remove(&participant_id);
                        break;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub async fn join_studio(
    ticket: &InviteTicket,
    name: String,
    participant_id: String,
) -> Result<PeerStream, String> {
    if ticket.version != 2 || ticket.iroh_ticket.trim().is_empty() {
        return Err(
            "This invite is from an older OpenPodcast build (LAN-only). Both host and guest need the latest app for cross-network studios."
                .into(),
        );
    }

    let addr = parse_iroh_addr(&ticket.iroh_ticket)?;
    if addr.relay_urls().next().is_none() && addr.ip_addrs().next().is_none() {
        return Err(
            "Invite has no network addresses. Ask the host to copy a fresh invite (host must be online)."
                .into(),
        );
    }
    if addr.relay_urls().next().is_none() {
        tracing::warn!("invite has no relay URL — cross-network join may fail");
    }

    let endpoint = Endpoint::bind(presets::N0)
        .await
        .map_err(|e| format!("Failed to start guest network: {e}"))?;
    // Guest also needs relay/STUN before dialing across NATs.
    wait_until_dialable(&endpoint).await?;

    let password = ticket.password.clone();
    let connect = async {
        let conn = endpoint
            .connect(addr, ALPN)
            .await
            .map_err(|e| format!("Could not reach host: {e}"))?;
        let (mut send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| format!("Could not open studio stream: {e}"))?;
        write_frame(
            &mut send,
            &PeerMessage::Hello {
                password,
                name,
                participant_id,
            },
        )
        .await?;
        Ok::<_, String>(PeerStream {
            _endpoint: endpoint.clone(),
            _conn: conn,
            send,
            _recv: recv,
        })
    };

    match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(
            "Timed out reaching the host. Ask them to Copy invite again while the studio is open, keep both apps online, and allow OpenPodcast through the firewall."
                .into(),
        ),
    }
}

fn parse_iroh_addr(s: &str) -> Result<EndpointAddr, String> {
    let ticket = EndpointTicket::from_str(s.trim())
        .map_err(|e| format!("Invalid iroh invite ticket: {e}"))?;
    Ok(ticket.endpoint_addr().clone())
}

pub fn encode_ticket(ticket: &InviteTicket) -> Result<String, String> {
    let json = serde_json::to_vec(ticket).map_err(|e| e.to_string())?;
    Ok(format!("opcast_{}", URL_SAFE_NO_PAD.encode(json)))
}

pub fn decode_ticket(s: &str) -> Result<InviteTicket, String> {
    let raw = s.trim();
    let b64 = raw
        .strip_prefix("opcast_")
        .ok_or_else(|| "Invalid invite: missing opcast_ prefix".to_string())?;
    let bytes = URL_SAFE_NO_PAD
        .decode(b64)
        .map_err(|e| format!("Invalid invite encoding: {e}"))?;

    // Peek version before full deserialize so v1 tickets get a clear message.
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
        let version = v.get("version").and_then(|x| x.as_u64()).unwrap_or(1);
        if version < 2 || v.get("iroh_ticket").and_then(|x| x.as_str()).unwrap_or("").is_empty()
        {
            return Err(
                "This invite is from an older OpenPodcast build (LAN-only). Both host and guest need the latest app for cross-network studios."
                    .into(),
            );
        }
    }

    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid invite payload: {e}"))
}

async fn write_frame(
    send: &mut iroh::endpoint::SendStream,
    msg: &PeerMessage,
) -> Result<(), String> {
    let data = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
    let len = (data.len() as u32).to_be_bytes();
    send.write_all(&len)
        .await
        .map_err(|e| e.to_string())?;
    send.write_all(&data)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn read_frame(recv: &mut iroh::endpoint::RecvStream) -> Result<PeerMessage, String> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .map_err(|e| e.to_string())?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        return Err("Frame too large".into());
    }
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&buf).map_err(|e| e.to_string())
}

pub async fn send_file(
    stream: &mut PeerStream,
    participant_id: &str,
    path: &PathBuf,
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let data = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let sha = hex::encode(hasher.finalize());
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("track.bin")
        .to_string();
    write_frame(
        &mut stream.send,
        &PeerMessage::FileOffer {
            participant_id: participant_id.into(),
            file_name: file_name.clone(),
            size: data.len() as u64,
            sha256: sha,
        },
    )
    .await?;
    const CHUNK: usize = 64 * 1024;
    for (i, chunk) in data.chunks(CHUNK).enumerate() {
        let offset = (i * CHUNK) as u64;
        write_frame(
            &mut stream.send,
            &PeerMessage::FileChunk {
                file_name: file_name.clone(),
                offset,
                data_b64: base64::engine::general_purpose::STANDARD.encode(chunk),
            },
        )
        .await?;
    }
    write_frame(
        &mut stream.send,
        &PeerMessage::FileDone {
            file_name: file_name.clone(),
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::StudioSettings;

    #[test]
    fn rejects_v1_lan_ticket() {
        let v1 = serde_json::json!({
            "version": 1,
            "host_addrs": ["192.168.0.1"],
            "port": 1234,
            "password": "x",
            "studio": {
                "name": "T",
                "mode": "audio_video",
                "frame_rate": 30,
                "resolution": "1080p"
            },
            "host_name": "Host",
            "created_at": "2026-11-02T00:00:00Z"
        });
        let enc = format!(
            "opcast_{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&v1).unwrap())
        );
        let err = decode_ticket(&enc).unwrap_err();
        assert!(err.contains("older OpenPodcast"), "{err}");
    }

    #[tokio::test]
    async fn host_guest_iroh_hello() {
        let studio = StudioSettings {
            name: "Test".into(),
            mode: crate::models::StudioMode::AudioVideo,
            frame_rate: 30,
            resolution: "1080p".into(),
        };
        let host = StudioHost::start(studio, "Host".into())
            .await
            .expect("host start");
        let invite = host.encode_ticket().expect("encode");
        let ticket = decode_ticket(&invite).expect("decode");
        assert_eq!(ticket.version, 2);
        assert!(!ticket.iroh_ticket.is_empty());

        let stream = join_studio(&ticket, "Guest".into(), "guest-1".into())
            .await
            .expect("guest join");
        // Give host accept loop a moment to register the peer
        tokio::time::sleep(Duration::from_millis(500)).await;
        let peers = host.peer_list();
        assert!(
            peers.iter().any(|p| p.id == "guest-1"),
            "expected guest in peer list: {peers:?}"
        );
        drop(stream);
        host.shutdown();
    }
}
