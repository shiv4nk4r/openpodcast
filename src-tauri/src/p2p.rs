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
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Application-Layer Protocol Negotiation for OpenPodcast studio sessions.
pub const ALPN: &[u8] = b"openpodcast/1";

/// Max guests in addition to the host (total people = 1 + MAX_GUESTS).
pub const MAX_GUESTS: usize = 3;

const ONLINE_TIMEOUT: Duration = Duration::from_secs(25);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerMessage {
    Hello {
        password: String,
        name: String,
        participant_id: String,
        /// Empty / "studio" = normal lobby join. "transfer" = deliver HQ tracks only.
        #[serde(default)]
        purpose: String,
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
        /// Guest resumes from this byte if a prior upload was interrupted.
        #[serde(default)]
        resume_from: u64,
    },
    FileChunk {
        file_name: String,
        offset: u64,
        data_b64: String,
    },
    FileDone {
        file_name: String,
    },
    /// Host → guest: bytes already on disk (optional; resume_from is primary).
    FileAck {
        file_name: String,
        received: u64,
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

/// Full studio roster (host + guests) for every participant's stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RosterEvent {
    pub peers: Vec<PeerInfo>,
}

/// Host → guest record control (start/stop HQ local capture).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordEvent {
    Start {
        session_id: String,
        started_at_ms: i64,
    },
    Stop,
}

/// Guest → host HQ track transfer progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferEvent {
    pub participant_id: String,
    pub file_name: String,
    pub status: String,
    /// Bytes sent for the current file (or overall when overall_* set).
    pub bytes_received: u64,
    pub total_bytes: u64,
    /// Aggregate across all files in this sync (preferred for UI %).
    #[serde(default)]
    pub overall_bytes: u64,
    #[serde(default)]
    pub overall_total: u64,
}

pub type PreviewBus = tokio::sync::broadcast::Sender<PreviewEvent>;
pub type RosterBus = tokio::sync::broadcast::Sender<RosterEvent>;
pub type RecordBus = tokio::sync::broadcast::Sender<RecordEvent>;
pub type TransferBus = tokio::sync::broadcast::Sender<TransferEvent>;
pub type GuestSend = Arc<tokio::sync::Mutex<iroh::endpoint::SendStream>>;

/// Bi-directional peer session over iroh QUIC.
pub struct PeerStream {
    pub(crate) _endpoint: Endpoint,
    pub(crate) _conn: iroh::endpoint::Connection,
    pub(crate) send: iroh::endpoint::SendStream,
    pub(crate) recv: iroh::endpoint::RecvStream,
}

/// Keeps the guest↔host QUIC session alive after join (Waiting / Admit).
pub struct GuestSession {
    _endpoint: Endpoint,
    _conn: iroh::endpoint::Connection,
    send: GuestSend,
    reader: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for GuestSession {
    fn drop(&mut self) {
        if let Some(h) = self.reader.take() {
            h.abort();
        }
    }
}

impl GuestSession {
    pub fn send_handle(&self) -> GuestSend {
        self.send.clone()
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
    /// Hold the connection open; forward host preview/admit/roster/record frames to UI buses.
    pub fn hold_open(
        self,
        preview_tx: PreviewBus,
        roster_tx: RosterBus,
        record_tx: RecordBus,
    ) -> GuestSession {
        let PeerStream {
            _endpoint,
            _conn,
            send,
            mut recv,
        } = self;
        let send = Arc::new(tokio::sync::Mutex::new(send));
        let reader = tokio::spawn(async move {
            loop {
                match read_frame(&mut recv).await {
                    Ok(PeerMessage::PreviewFrame {
                        participant_id,
                        jpeg_b64,
                        pcm_b64,
                    }) => {
                        let _ = preview_tx.send(PreviewEvent {
                            participant_id,
                            jpeg_b64,
                            pcm_b64,
                        });
                    }
                    Ok(PeerMessage::Admit { participant_id }) => {
                        let _ = preview_tx.send(PreviewEvent {
                            participant_id: format!("__admit__:{participant_id}"),
                            jpeg_b64: None,
                            pcm_b64: None,
                        });
                    }
                    Ok(PeerMessage::PeerList { peers }) => {
                        let _ = roster_tx.send(RosterEvent { peers });
                    }
                    Ok(PeerMessage::RecordStart {
                        session_id,
                        started_at_ms,
                    }) => {
                        let _ = record_tx.send(RecordEvent::Start {
                            session_id,
                            started_at_ms,
                        });
                    }
                    Ok(PeerMessage::RecordStop) => {
                        let _ = record_tx.send(RecordEvent::Stop);
                    }
                    Ok(PeerMessage::FileAck { .. }) => {
                        // Consumed by send_file_on while holding the send side;
                        // acks during idle are ignored.
                    }
                    Ok(PeerMessage::Leave { .. }) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
        GuestSession {
            _endpoint,
            _conn,
            send,
            reader: Some(reader),
        }
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
    guest_sends: Arc<Mutex<HashMap<String, GuestSend>>>,
    /// Host session folder that receives guest HQ tracks during/after a take.
    track_inbox: Arc<Mutex<Option<PathBuf>>>,
    _preview_tx: PreviewBus,
    _roster_tx: RosterBus,
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
        roster_tx: RosterBus,
        transfer_tx: TransferBus,
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
        let guest_sends = Arc::new(Mutex::new(HashMap::new()));
        let track_inbox = Arc::new(Mutex::new(None));
        let (tx, mut rx) = mpsc::channel::<()>(1);
        let peers_c = peers.clone();
        let guest_sends_c = guest_sends.clone();
        let track_inbox_c = track_inbox.clone();
        let transfer_tx_c = transfer_tx.clone();
        let password_c = password.clone();
        let host_name_c = Arc::new(Mutex::new(host_name));
        let host_name_accept = host_name_c.clone();
        let preview_tx_c = preview_tx.clone();
        let roster_tx_c = roster_tx.clone();
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
                        let guest_sends = guest_sends_c.clone();
                        let track_inbox = track_inbox_c.clone();
                        let transfer_tx = transfer_tx_c.clone();
                        let password = password_c.clone();
                        let host_name = host_name_accept.lock().clone();
                        let live_host_name = host_name_accept.clone();
                        let preview_tx = preview_tx_c.clone();
                        let roster_tx = roster_tx_c.clone();
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
                                        live_host_name,
                                        peers,
                                        guest_sends,
                                        track_inbox,
                                        transfer_tx,
                                        preview_tx,
                                        roster_tx,
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
            guest_sends,
            track_inbox,
            _preview_tx: preview_tx,
            _roster_tx: roster_tx,
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

    pub fn admit(&self, participant_id: &str) -> Result<(), String> {
        let mut peers = self.peers.lock();
        let admitted = peers.values().filter(|p| p.admitted).count();
        let already = peers.get(participant_id).map(|p| p.admitted).unwrap_or(false);
        if !already && admitted >= MAX_GUESTS {
            return Err(format!(
                "Studio is full (max {MAX_GUESTS} guests / {} people).",
                MAX_GUESTS + 1
            ));
        }
        if let Some(p) = peers.get_mut(participant_id) {
            p.admitted = true;
            Ok(())
        } else {
            Err("Guest not found".into())
        }
    }

    pub fn peer_list(&self) -> Vec<PeerInfo> {
        self.peers.lock().values().cloned().collect()
    }

    pub fn roster_with_host(&self) -> Vec<PeerInfo> {
        build_roster(&self.peers, &self.live_host_name.lock())
    }

    pub fn guest_send_handles(&self) -> Vec<GuestSend> {
        self.guest_sends.lock().values().cloned().collect()
    }

    pub fn admitted_guest_ids(&self) -> Vec<String> {
        self.peers
            .lock()
            .values()
            .filter(|p| p.admitted)
            .map(|p| p.id.clone())
            .collect()
    }

    pub fn set_track_inbox(&self, dir: Option<PathBuf>) {
        *self.track_inbox.lock() = dir;
    }

    pub async fn broadcast_record_start(&self, session_id: String, started_at_ms: i64) {
        let targets = self.guest_send_handles();
        let msg = PeerMessage::RecordStart {
            session_id,
            started_at_ms,
        };
        for send in targets {
            let mut guard = send.lock().await;
            if let Err(e) = write_frame(&mut guard, &msg).await {
                tracing::warn!("RecordStart send failed: {e}");
            }
        }
    }

    pub async fn broadcast_record_stop(&self) {
        let targets = self.guest_send_handles();
        let msg = PeerMessage::RecordStop;
        for send in targets {
            let mut guard = send.lock().await;
            if let Err(e) = write_frame(&mut guard, &msg).await {
                tracing::warn!("RecordStop send failed: {e}");
            }
        }
    }

    pub fn shutdown(&self) {
        if let Some(tx) = self.shutdown.lock().take() {
            let _ = tx.try_send(());
        }
        self.guest_sends.lock().clear();
        *self.track_inbox.lock() = None;
    }
}

fn build_roster(peers: &Mutex<HashMap<String, PeerInfo>>, host_name: &str) -> Vec<PeerInfo> {
    let mut list = vec![PeerInfo {
        id: "host".into(),
        name: host_name.to_string(),
        role: "Host".into(),
        admitted: true,
    }];
    list.extend(peers.lock().values().cloned());
    list
}

async fn publish_roster(
    peers: &Mutex<HashMap<String, PeerInfo>>,
    guest_sends: &Mutex<HashMap<String, GuestSend>>,
    host_name: &str,
    roster_tx: &RosterBus,
) {
    let list = build_roster(peers, host_name);
    let _ = roster_tx.send(RosterEvent {
        peers: list.clone(),
    });
    let targets: Vec<GuestSend> = guest_sends.lock().values().cloned().collect();
    let msg = PeerMessage::PeerList { peers: list };
    for send in targets {
        let mut guard = send.lock().await;
        if let Err(e) = write_frame(&mut guard, &msg).await {
            tracing::warn!("roster send failed: {e}");
        }
    }
}

async fn forward_preview_except(
    guest_sends: &Mutex<HashMap<String, GuestSend>>,
    except_id: &str,
    participant_id: String,
    jpeg_b64: Option<String>,
    pcm_b64: Option<String>,
) {
    let targets: Vec<GuestSend> = guest_sends
        .lock()
        .iter()
        .filter(|(id, _)| id.as_str() != except_id)
        .map(|(_, s)| s.clone())
        .collect();
    if targets.is_empty() {
        return;
    }
    let msg = PeerMessage::PreviewFrame {
        participant_id,
        jpeg_b64,
        pcm_b64,
    };
    for send in targets {
        let mut guard = send.lock().await;
        if let Err(e) = write_frame(&mut guard, &msg).await {
            tracing::warn!("forward preview failed: {e}");
        }
    }
}

fn safe_track_file_name(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if base.is_empty() || base.contains("..") {
        return None;
    }
    let ok = (base.contains("-audio.") || base.contains("-video."))
        && (base.ends_with(".wav")
            || base.ends_with(".webm")
            || base.ends_with(".mp4")
            || base.ends_with(".mkv"));
    ok.then(|| base.to_string())
}

async fn handle_guest(
    conn: iroh::endpoint::Connection,
    password: String,
    host_name: String,
    live_host_name: Arc<Mutex<String>>,
    peers: Arc<Mutex<HashMap<String, PeerInfo>>>,
    guest_sends: Arc<Mutex<HashMap<String, GuestSend>>>,
    track_inbox: Arc<Mutex<Option<PathBuf>>>,
    transfer_tx: TransferBus,
    preview_tx: PreviewBus,
    roster_tx: RosterBus,
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
            purpose,
        } => {
            if pw != password {
                let mut s = send.lock().await;
                write_frame(
                    &mut s,
                    &PeerMessage::Deny {
                        reason: "Invalid studio password".into(),
                    },
                )
                .await?;
                return Ok(());
            }

            let transfer_only = purpose == "transfer";
            if transfer_only {
                if track_inbox.lock().is_none() {
                    let mut s = send.lock().await;
                    write_frame(
                        &mut s,
                        &PeerMessage::Deny {
                            reason: "Host is not collecting tracks for this session right now."
                                .into(),
                        },
                    )
                    .await?;
                    return Ok(());
                }
                tracing::info!(%participant_id, %name, "guest transfer hello");
                guest_sends
                    .lock()
                    .insert(participant_id.clone(), send.clone());
                {
                    let mut s = send.lock().await;
                    write_frame(
                        &mut s,
                        &PeerMessage::Admit {
                            participant_id: participant_id.clone(),
                        },
                    )
                    .await?;
                }
            } else {
                if peers.lock().len() >= MAX_GUESTS {
                    let mut s = send.lock().await;
                    write_frame(
                        &mut s,
                        &PeerMessage::Deny {
                            reason: format!("Studio is full (max {MAX_GUESTS} guests)."),
                        },
                    )
                    .await?;
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
                guest_sends
                    .lock()
                    .insert(participant_id.clone(), send.clone());
                {
                    let mut s = send.lock().await;
                    write_frame(
                        &mut s,
                        &PeerMessage::Waiting {
                            participant_id: participant_id.clone(),
                            name,
                            host_name: host_name.clone(),
                        },
                    )
                    .await?;
                }
                {
                    let name = live_host_name.lock().clone();
                    publish_roster(&peers, &guest_sends, &name, &roster_tx).await;
                }
                loop {
                    let admitted = peers
                        .lock()
                        .get(&participant_id)
                        .map(|p| p.admitted)
                        .unwrap_or(false);
                    if admitted {
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
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                {
                    let name = live_host_name.lock().clone();
                    publish_roster(&peers, &guest_sends, &name, &roster_tx).await;
                }
            }

            // Shared loop: live studio media + (resumable) HQ track receive.
            let mut incoming: HashMap<String, (std::fs::File, u64, u64, String)> = HashMap::new();
            loop {
                match read_frame(&mut recv).await {
                    Ok(PeerMessage::Leave { .. }) => {
                        peers.lock().remove(&participant_id);
                        guest_sends.lock().remove(&participant_id);
                        if !transfer_only {
                            let name = live_host_name.lock().clone();
                            publish_roster(&peers, &guest_sends, &name, &roster_tx).await;
                        }
                        break;
                    }
                    Ok(PeerMessage::PreviewFrame {
                        participant_id: pid,
                        jpeg_b64,
                        pcm_b64,
                    }) => {
                        if transfer_only {
                            continue;
                        }
                        let _ = preview_tx.send(PreviewEvent {
                            participant_id: pid.clone(),
                            jpeg_b64: jpeg_b64.clone(),
                            pcm_b64: pcm_b64.clone(),
                        });
                        forward_preview_except(
                            &guest_sends,
                            &pid,
                            pid.clone(),
                            jpeg_b64,
                            pcm_b64,
                        )
                        .await;
                    }
                    Ok(PeerMessage::FileOffer {
                        participant_id: pid,
                        file_name,
                        size,
                        sha256: _,
                        resume_from,
                    }) => {
                        let Some(safe) = safe_track_file_name(&file_name) else {
                            tracing::warn!("rejecting unsafe track name {file_name}");
                            continue;
                        };
                        let Some(dir) = track_inbox.lock().clone() else {
                            tracing::warn!("track inbox not set; dropping {safe}");
                            continue;
                        };
                        let dest = dir.join(&safe);
                        let existing = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                        let start = resume_from.min(existing).min(size);
                        match open_track_file(&dest, start) {
                            Ok(f) => {
                                incoming.insert(safe.clone(), (f, size, start, pid.clone()));
                                {
                                    let mut s = send.lock().await;
                                    let _ = write_frame(
                                        &mut s,
                                        &PeerMessage::FileAck {
                                            file_name: safe.clone(),
                                            received: start,
                                        },
                                    )
                                    .await;
                                }
                                let _ = transfer_tx.send(TransferEvent {
                                    participant_id: pid,
                                    file_name: safe,
                                    status: "receiving".into(),
                                    bytes_received: start,
                                    total_bytes: size,
                                    overall_bytes: start,
                                    overall_total: size,
                                });
                            }
                            Err(e) => tracing::warn!("open track file failed: {e}"),
                        }
                    }
                    Ok(PeerMessage::FileChunk {
                        file_name,
                        offset: _,
                        data_b64,
                    }) => {
                        let Some(safe) = safe_track_file_name(&file_name) else {
                            continue;
                        };
                        if let Some((file, total, received, pid)) = incoming.get_mut(&safe) {
                            match base64::engine::general_purpose::STANDARD.decode(&data_b64) {
                                Ok(bytes) => {
                                    if let Err(e) = file.write_all(&bytes) {
                                        tracing::warn!("write track chunk failed: {e}");
                                        continue;
                                    }
                                    *received += bytes.len() as u64;
                                    let _ = transfer_tx.send(TransferEvent {
                                        participant_id: pid.clone(),
                                        file_name: safe,
                                        status: "receiving".into(),
                                        bytes_received: *received,
                                        total_bytes: *total,
                                        overall_bytes: *received,
                                        overall_total: *total,
                                    });
                                }
                                Err(e) => tracing::warn!("bad track chunk: {e}"),
                            }
                        }
                    }
                    Ok(PeerMessage::FileDone { file_name }) => {
                        let Some(safe) = safe_track_file_name(&file_name) else {
                            continue;
                        };
                        if let Some((mut file, total, received, pid)) = incoming.remove(&safe) {
                            let _ = file.flush();
                            let _ = transfer_tx.send(TransferEvent {
                                participant_id: pid,
                                file_name: safe,
                                status: "done".into(),
                                bytes_received: received,
                                total_bytes: total,
                                overall_bytes: received,
                                overall_total: total,
                            });
                        }
                    }
                    Ok(_) => {}
                    Err(_) => {
                        peers.lock().remove(&participant_id);
                        guest_sends.lock().remove(&participant_id);
                        if !transfer_only {
                            let name = live_host_name.lock().clone();
                            publish_roster(&peers, &guest_sends, &name, &roster_tx).await;
                        }
                        break;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn open_track_file(path: &PathBuf, resume_from: u64) -> Result<std::fs::File, String> {
    use std::io::{Seek, SeekFrom};
    if resume_from == 0 {
        return std::fs::File::create(path).map_err(|e| e.to_string());
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    f.set_len(resume_from).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(resume_from))
        .map_err(|e| e.to_string())?;
    Ok(f)
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
                purpose: String::new(),
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

/// Reconnect only to push HQ tracks — no lobby seat required.
pub async fn join_for_transfer(
    ticket: &InviteTicket,
    name: String,
    participant_id: String,
) -> Result<PeerStream, String> {
    if ticket.version != 2 || ticket.iroh_ticket.trim().is_empty() {
        return Err("Need a current invite while the host is still collecting tracks.".into());
    }
    let addr = parse_iroh_addr(&ticket.iroh_ticket)?;
    let endpoint = Endpoint::bind(presets::N0)
        .await
        .map_err(|e| format!("Failed to start transfer network: {e}"))?;
    wait_until_dialable(&endpoint).await?;
    let password = ticket.password.clone();
    let connect = async {
        let conn = endpoint
            .connect(addr, ALPN)
            .await
            .map_err(|e| format!("Could not reach host: {e}"))?;
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| format!("Could not open transfer stream: {e}"))?;
        write_frame(
            &mut send,
            &PeerMessage::Hello {
                password,
                name,
                participant_id: participant_id.clone(),
                purpose: "transfer".into(),
            },
        )
        .await?;
        match read_frame(&mut recv).await? {
            PeerMessage::Admit { .. } => {}
            PeerMessage::Deny { reason } => return Err(reason),
            other => {
                return Err(format!("Unexpected host response for transfer: {other:?}"));
            }
        }
        Ok(PeerStream {
            _endpoint: endpoint.clone(),
            _conn: conn,
            send,
            recv,
        })
    };
    match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(
            "Timed out reaching host for track delivery. Host must keep the studio open (or reopen collection)."
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

pub async fn fanout_message(targets: Vec<GuestSend>, msg: PeerMessage) {
    for send in targets {
        let mut guard = send.lock().await;
        if let Err(e) = write_frame(&mut guard, &msg).await {
            tracing::warn!("fanout failed: {e}");
        }
    }
}

pub(crate) async fn write_frame(
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

pub async fn send_file_on(
    send: GuestSend,
    participant_id: &str,
    path: &PathBuf,
    resume_from: u64,
    progress_key: Option<(String, String)>,
    // (bus, bytes already completed before this file, grand total of all files)
    progress_tx: Option<(TransferBus, u64, u64)>,
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| format!("track stat: {e}"))?;
    let size = meta.len();
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("track.bin")
        .to_string();
    let resume_from = resume_from.min(size);

    // Stream hash then stream send — keep multi-GB podcasts off the heap.
    let mut hasher = Sha256::new();
    {
        let mut f = tokio::fs::File::open(path)
            .await
            .map_err(|e| format!("open track: {e}"))?;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = f.read(&mut buf).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
    }
    let sha = hex::encode(hasher.finalize());

    let mut guard = send.lock().await;
    write_frame(
        &mut guard,
        &PeerMessage::FileOffer {
            participant_id: participant_id.into(),
            file_name: file_name.clone(),
            size,
            sha256: sha,
            resume_from,
        },
    )
    .await?;

    if resume_from >= size {
        write_frame(
            &mut guard,
            &PeerMessage::FileDone {
                file_name: file_name.clone(),
            },
        )
        .await?;
        return Ok(());
    }

    let mut f = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("open track: {e}"))?;
    f.seek(std::io::SeekFrom::Start(resume_from))
        .await
        .map_err(|e| e.to_string())?;
    // 256 KiB chunks — fewer JSON frames for hour-long takes.
    let mut buf = vec![0u8; 256 * 1024];
    let mut offset = resume_from;
    let mut since_persist = 0u64;
    let emit = |bytes: u64, status: &str| {
        if let Some((tx, base, grand)) = progress_tx.as_ref() {
            let overall = base.saturating_add(bytes);
            let _ = tx.send(TransferEvent {
                participant_id: participant_id.to_string(),
                file_name: file_name.clone(),
                status: status.into(),
                bytes_received: bytes,
                total_bytes: size,
                overall_bytes: overall,
                overall_total: (*grand).max(1),
            });
        }
    };
    emit(resume_from, "sending");
    loop {
        let n = f.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        write_frame(
            &mut guard,
            &PeerMessage::FileChunk {
                file_name: file_name.clone(),
                offset,
                data_b64: base64::engine::general_purpose::STANDARD.encode(&buf[..n]),
            },
        )
        .await?;
        offset += n as u64;
        since_persist += n as u64;
        emit(offset, "sending");
        // Persist resume cursor every ~8 MiB so a drop mid-podcast can continue.
        if since_persist >= 8 * 1024 * 1024 {
            since_persist = 0;
            if let Some((id, name)) = progress_key.as_ref() {
                let _ = crate::transfer_queue::update_offset(id, name, offset);
            }
        }
    }
    if let Some((id, name)) = progress_key.as_ref() {
        let _ = crate::transfer_queue::update_offset(id, name, offset);
    }
    write_frame(
        &mut guard,
        &PeerMessage::FileDone {
            file_name: file_name.clone(),
        },
    )
    .await?;
    emit(size, "done");
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
        let (roster_tx, _roster_rx) = tokio::sync::broadcast::channel(8);
        let (transfer_tx, _transfer_rx) = tokio::sync::broadcast::channel(8);
        let (record_tx, _record_rx) = tokio::sync::broadcast::channel(8);
        let host = StudioHost::start(
            studio,
            "Host".into(),
            preview_tx.clone(),
            roster_tx.clone(),
            transfer_tx,
        )
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
        let _session = stream.hold_open(preview_tx, roster_tx, record_tx);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            host.peer_list().iter().any(|p| p.id == "guest-1"),
            "guest must stay visible while session is held open"
        );
        host.admit("guest-1").expect("admit");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            host.peer_list()
                .iter()
                .any(|p| p.id == "guest-1" && p.admitted),
            "expected admitted guest: {:?}",
            host.peer_list()
        );
        let roster = host.roster_with_host();
        assert!(roster.iter().any(|p| p.id == "host"));
        assert!(roster.iter().any(|p| p.id == "guest-1" && p.admitted));

        // Fill remaining guest slots, then admit must refuse a 4th guest.
        for i in 2..=MAX_GUESTS {
            let id = format!("guest-{i}");
            host.peers.lock().insert(
                id.clone(),
                PeerInfo {
                    id: id.clone(),
                    name: format!("Guest {i}"),
                    role: "Guest".into(),
                    admitted: false,
                },
            );
            host.admit(&id).expect("admit within cap");
        }
        host.peers.lock().insert(
            "guest-overflow".into(),
            PeerInfo {
                id: "guest-overflow".into(),
                name: "Too Many".into(),
                role: "Guest".into(),
                admitted: false,
            },
        );
        let err = host.admit("guest-overflow").expect_err("4th guest blocked");
        assert!(err.contains("full"), "{err}");

        drop(_session);
        host.shutdown();
    }
}
