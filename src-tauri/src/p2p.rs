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
        /// Host display name (so guests see the real name, not a stale ticket).
        #[serde(default)]
        host_name: String,
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

/// Live studio preview payload pushed to the UI over a broadcast bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewEvent {
    pub participant_id: String,
    pub jpeg_b64: Option<String>,
    pub pcm_b64: Option<String>,
}

pub type PreviewBus = tokio::sync::broadcast::Sender<PreviewEvent>;
pub type GuestSend = Arc<tokio::sync::Mutex<iroh::endpoint::SendStream>>;

/// Bi-directional peer session over iroh QUIC.
pub struct PeerStream {
    _endpoint: Endpoint,
    _conn: iroh::endpoint::Connection,
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
}

/// Keeps the guest↔host QUIC session alive after join (Waiting / Admit).
/// Video (JPEG) and audio (PCM) use separate bi-streams so camera frames
/// cannot stall microphone packets.
pub struct GuestSession {
    _endpoint: Endpoint,
    _conn: iroh::endpoint::Connection,
    video_send: GuestSend,
    audio_send: GuestSend,
    readers: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for GuestSession {
    fn drop(&mut self) {
        for h in self.readers.drain(..) {
            h.abort();
        }
    }
}

impl GuestSession {
    pub fn video_send_handle(&self) -> GuestSend {
        self.video_send.clone()
    }

    pub fn audio_send_handle(&self) -> GuestSend {
        self.audio_send.clone()
    }
}

pub async fn send_preview_on(
    send: GuestSend,
    participant_id: String,
    jpeg_b64: Option<String>,
    pcm_b64: Option<String>,
) -> Result<(), String> {
    let mut guard = send.lock().await;
    write_frame(
        &mut guard,
        &PeerMessage::PreviewFrame {
            participant_id,
            jpeg_b64,
            pcm_b64,
        },
    )
    .await
}

impl PeerStream {
    /// Hold the connection open; open a dedicated audio stream; forward frames to UI.
    pub async fn hold_open(self, preview_tx: PreviewBus) -> Result<GuestSession, String> {
        let PeerStream {
            _endpoint,
            _conn,
            send,
            mut recv,
        } = self;
        let video_send = Arc::new(tokio::sync::Mutex::new(send));
        let preview_video = preview_tx.clone();
        let video_reader = tokio::spawn(async move {
            loop {
                match read_frame(&mut recv).await {
                    Ok(PeerMessage::PreviewFrame {
                        participant_id,
                        jpeg_b64,
                        pcm_b64,
                    }) => {
                        let _ = preview_video.send(PreviewEvent {
                            participant_id,
                            jpeg_b64,
                            pcm_b64,
                        });
                    }
                    Ok(PeerMessage::Admit { participant_id }) => {
                        let _ = preview_video.send(PreviewEvent {
                            participant_id: format!("__admit__:{participant_id}"),
                            jpeg_b64: None,
                            pcm_b64: None,
                        });
                    }
                    Ok(PeerMessage::Leave { .. }) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });

        // Dedicated audio bi-stream (host accept_bi runs during Waiting/Admit).
        let (audio_send_raw, mut audio_recv) = _conn
            .open_bi()
            .await
            .map_err(|e| format!("Could not open audio stream: {e}"))?;
        let audio_send = Arc::new(tokio::sync::Mutex::new(audio_send_raw));
        let preview_audio = preview_tx;
        let audio_reader = tokio::spawn(async move {
            loop {
                match read_frame(&mut audio_recv).await {
                    Ok(PeerMessage::PreviewFrame {
                        participant_id,
                        jpeg_b64,
                        pcm_b64,
                    }) => {
                        let _ = preview_audio.send(PreviewEvent {
                            participant_id,
                            jpeg_b64,
                            pcm_b64,
                        });
                    }
                    Ok(PeerMessage::Leave { .. }) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });

        Ok(GuestSession {
            _endpoint,
            _conn,
            video_send,
            audio_send,
            readers: vec![video_reader, audio_reader],
        })
    }
}

pub struct StudioHost {
    pub password: String,
    pub ticket: InviteTicket,
    pub peers: Arc<Mutex<HashMap<String, PeerInfo>>>,
    endpoint: Endpoint,
    shutdown: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    /// Shared with the accept loop so renamed hosts show up for new joiners.
    live_host_name: Arc<Mutex<String>>,
    guest_video_sends: Arc<Mutex<HashMap<String, GuestSend>>>,
    guest_audio_sends: Arc<Mutex<HashMap<String, GuestSend>>>,
    _preview_tx: PreviewBus,
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
    pub async fn start(
        studio: StudioSettings,
        host_name: String,
        preview_tx: PreviewBus,
    ) -> Result<Self, String> {
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
            host_name: host_name.clone(),
            created_at: Utc::now(),
        };

        let peers = Arc::new(Mutex::new(HashMap::new()));
        let guest_video_sends = Arc::new(Mutex::new(HashMap::new()));
        let guest_audio_sends = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::channel::<()>(1);
        let peers_c = peers.clone();
        let guest_video_c = guest_video_sends.clone();
        let guest_audio_c = guest_audio_sends.clone();
        let password_c = password.clone();
        let host_name_c = Arc::new(Mutex::new(host_name));
        let host_name_accept = host_name_c.clone();
        let preview_tx_c = preview_tx.clone();
        let ep_accept = endpoint.clone();
        let ep_keepalive = endpoint.clone();

        // Keep the relay registration warm so invites stay dialable.
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(20)).await;
                let _ = tokio::time::timeout(Duration::from_secs(8), ep_keepalive.online()).await;
            }
        });

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = rx.recv() => {
                        ep_accept.close().await;
                        break;
                    }
                    incoming = ep_accept.accept() => {
                        let Some(incoming) = incoming else { break };
                        let peers = peers_c.clone();
                        let guest_video_sends = guest_video_c.clone();
                        let guest_audio_sends = guest_audio_c.clone();
                        let password = password_c.clone();
                        let host_name = host_name_accept.lock().clone();
                        let preview_tx = preview_tx_c.clone();
                        tokio::spawn(async move {
                            let accepting = match incoming.accept() {
                                Ok(a) => a,
                                Err(e) => {
                                    tracing::warn!("guest accept failed: {e}");
                                    return;
                                }
                            };
                            match accepting.await {
                                Ok(conn) => {
                                    if let Err(e) = handle_guest(
                                        conn,
                                        password,
                                        host_name,
                                        peers,
                                        guest_video_sends,
                                        guest_audio_sends,
                                        preview_tx,
                                    )
                                    .await
                                    {
                                        tracing::warn!("guest session ended: {e}");
                                    }
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
            live_host_name: host_name_c,
            guest_video_sends,
            guest_audio_sends,
            _preview_tx: preview_tx,
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

    pub fn set_host_name(&mut self, host_name: String) {
        *self.live_host_name.lock() = host_name.clone();
        self.ticket.host_name = host_name;
    }

    pub fn admit(&self, participant_id: &str) {
        if let Some(p) = self.peers.lock().get_mut(participant_id) {
            p.admitted = true;
        }
    }

    pub fn peer_list(&self) -> Vec<PeerInfo> {
        self.peers.lock().values().cloned().collect()
    }

    pub fn guest_video_handles(&self) -> Vec<GuestSend> {
        self.guest_video_sends.lock().values().cloned().collect()
    }

    pub fn guest_audio_handles(&self) -> Vec<GuestSend> {
        self.guest_audio_sends.lock().values().cloned().collect()
    }

    pub fn shutdown(&self) {
        if let Some(tx) = self.shutdown.lock().take() {
            let _ = tx.try_send(());
        }
        self.guest_video_sends.lock().clear();
        self.guest_audio_sends.lock().clear();
    }
}

async fn handle_guest(
    conn: iroh::endpoint::Connection,
    password: String,
    host_name: String,
    peers: Arc<Mutex<HashMap<String, PeerInfo>>>,
    guest_video_sends: Arc<Mutex<HashMap<String, GuestSend>>>,
    guest_audio_sends: Arc<Mutex<HashMap<String, GuestSend>>>,
    preview_tx: PreviewBus,
) -> Result<(), String> {
    let (send, mut recv) = conn
        .accept_bi()
        .await
        .map_err(|e| format!("accept stream: {e}"))?;
    let send = Arc::new(tokio::sync::Mutex::new(send));

    let msg = read_frame(&mut recv).await?;
    match msg {
        PeerMessage::Hello {
            password: pw,
            name,
            participant_id,
        } => {
            if pw != password {
                {
                    let mut s = send.lock().await;
                    write_frame(
                        &mut s,
                        &PeerMessage::Deny {
                            reason: "Invalid studio password".into(),
                        },
                    )
                    .await?;
                }
                return Ok(());
            }
            tracing::info!(%participant_id, %name, "guest hello — waiting for admit");
            peers.lock().insert(
                participant_id.clone(),
                PeerInfo {
                    id: participant_id.clone(),
                    name: name.clone(),
                    role: "Guest".into(),
                    admitted: false,
                },
            );
            guest_video_sends
                .lock()
                .insert(participant_id.clone(), send.clone());
            {
                let mut s = send.lock().await;
                write_frame(
                    &mut s,
                    &PeerMessage::Waiting {
                        participant_id: participant_id.clone(),
                        name,
                        host_name,
                    },
                )
                .await?;
            }

            // Accept the dedicated audio bi-stream while waiting for Admit.
            let mut audio_accept = Box::pin(conn.accept_bi());
            let mut audio_pair: Option<(
                iroh::endpoint::SendStream,
                iroh::endpoint::RecvStream,
            )> = None;
            loop {
                let admitted = peers
                    .lock()
                    .get(&participant_id)
                    .map(|p| p.admitted)
                    .unwrap_or(false);
                if admitted && audio_pair.is_some() {
                    let mut s = send.lock().await;
                    write_frame(
                        &mut s,
                        &PeerMessage::Admit {
                            participant_id: participant_id.clone(),
                        },
                    )
                    .await?;
                    break;
                }
                tokio::select! {
                    res = &mut audio_accept, if audio_pair.is_none() => {
                        match res {
                            Ok(pair) => audio_pair = Some(pair),
                            Err(e) => return Err(format!("accept audio stream: {e}")),
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                }
            }

            let (audio_send_raw, mut audio_recv) = audio_pair
                .ok_or_else(|| "Guest never opened an audio stream".to_string())?;
            let audio_send = Arc::new(tokio::sync::Mutex::new(audio_send_raw));
            guest_audio_sends
                .lock()
                .insert(participant_id.clone(), audio_send);

            let preview_audio = preview_tx.clone();
            let audio_pid = participant_id.clone();
            let peers_audio = peers.clone();
            let video_map = guest_video_sends.clone();
            let audio_map = guest_audio_sends.clone();
            let audio_reader = tokio::spawn(async move {
                loop {
                    match read_frame(&mut audio_recv).await {
                        Ok(PeerMessage::PreviewFrame {
                            participant_id: pid,
                            jpeg_b64,
                            pcm_b64,
                        }) => {
                            let _ = preview_audio.send(PreviewEvent {
                                participant_id: pid,
                                jpeg_b64,
                                pcm_b64,
                            });
                        }
                        Ok(PeerMessage::Leave { .. }) | Err(_) => {
                            peers_audio.lock().remove(&audio_pid);
                            video_map.lock().remove(&audio_pid);
                            audio_map.lock().remove(&audio_pid);
                            break;
                        }
                        Ok(_) => {}
                    }
                }
            });

            loop {
                match read_frame(&mut recv).await {
                    Ok(PeerMessage::Leave { .. }) => {
                        peers.lock().remove(&participant_id);
                        guest_video_sends.lock().remove(&participant_id);
                        guest_audio_sends.lock().remove(&participant_id);
                        break;
                    }
                    Ok(PeerMessage::PreviewFrame {
                        participant_id: pid,
                        jpeg_b64,
                        pcm_b64,
                    }) => {
                        let _ = preview_tx.send(PreviewEvent {
                            participant_id: pid,
                            jpeg_b64,
                            pcm_b64,
                        });
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
                        guest_video_sends.lock().remove(&participant_id);
                        guest_audio_sends.lock().remove(&participant_id);
                        break;
                    }
                }
            }
            audio_reader.abort();
        }
        _ => {}
    }
    Ok(())
}

pub async fn join_studio(
    ticket: &InviteTicket,
    name: String,
    participant_id: String,
) -> Result<(PeerStream, String), String> {
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
        // Wait until the host registers us (do not drop before this — that raced
        // and left both sides thinking nobody else was in the studio).
        let mut recv = recv;
        let host_name = match read_frame(&mut recv).await? {
            PeerMessage::Waiting { host_name, .. } => {
                if host_name.trim().is_empty() {
                    ticket.host_name.clone()
                } else {
                    host_name
                }
            }
            PeerMessage::Deny { reason } => {
                return Err(reason);
            }
            other => {
                return Err(format!("Unexpected host response while joining: {other:?}"));
            }
        };
        Ok::<_, String>((
            PeerStream {
                _endpoint: endpoint.clone(),
                _conn: conn,
                send,
                recv,
            },
            host_name,
        ))
    };

    match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
        Ok(Ok(pair)) => Ok(pair),
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

pub async fn fanout_preview(
    targets: Vec<GuestSend>,
    participant_id: String,
    jpeg_b64: Option<String>,
    pcm_b64: Option<String>,
) -> Result<(), String> {
    if targets.is_empty() {
        return Ok(());
    }
    let msg = PeerMessage::PreviewFrame {
        participant_id,
        jpeg_b64,
        pcm_b64,
    };
    for send in targets {
        let mut guard = send.lock().await;
        if let Err(e) = write_frame(&mut guard, &msg).await {
            tracing::warn!("preview send to guest failed: {e}");
        }
    }
    Ok(())
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
        let (preview_tx, _preview_rx) = tokio::sync::broadcast::channel(16);
        let host = StudioHost::start(studio, "Host".into(), preview_tx.clone())
            .await
            .expect("host start");
        let invite = host.encode_ticket().expect("encode");
        let ticket = decode_ticket(&invite).expect("decode");
        assert_eq!(ticket.version, 2);
        assert!(!ticket.iroh_ticket.is_empty());

        let (stream, host_name) = join_studio(&ticket, "Guest".into(), "guest-1".into())
            .await
            .expect("guest join");
        assert_eq!(host_name, "Host");
        // Give host accept loop a moment to register the peer
        tokio::time::sleep(Duration::from_millis(500)).await;
        let peers = host.peer_list();
        assert!(
            peers.iter().any(|p| p.id == "guest-1" && !p.admitted),
            "expected waiting guest in peer list: {peers:?}"
        );
        let _session = stream
            .hold_open(preview_tx)
            .await
            .expect("hold open");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            host.peer_list().iter().any(|p| p.id == "guest-1"),
            "guest must stay visible while session is held open"
        );
        host.admit("guest-1");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            host.peer_list()
                .iter()
                .any(|p| p.id == "guest-1" && p.admitted),
            "expected admitted guest: {:?}",
            host.peer_list()
        );
        drop(_session);
        host.shutdown();
    }
}
