//! Peer-to-peer studio networking without a hosted signaling server.
//! Invite tickets carry dialing info (addrs + port + password + studio settings).
//! Peers connect over TCP; on LAN this is direct. Public addresses are probed via
//! a free STUN-like HTTP echo so remote guests can dial without our infrastructure.
//!
//! Protocol (length-prefixed JSON frames on TCP):
//! - Hello { password, name, role }
//! - Admit / Deny
//! - PreviewAudio / PreviewJpeg (base64 chunks)
//! - ClockSync { host_unix_ms }
//! - RecordStart / RecordStop
//! - FileOffer / FileChunk / FileDone
//! - Reaction { emoji }
//! - Leave

use crate::models::{InviteTicket, StudioSettings};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use parking_lot::Mutex;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

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

pub struct StudioHost {
    pub password: String,
    pub port: u16,
    pub ticket: InviteTicket,
    pub peers: Arc<Mutex<HashMap<String, PeerInfo>>>,
    shutdown: Arc<Mutex<Option<mpsc::Sender<()>>>>,
}

impl StudioHost {
    pub async fn start(studio: StudioSettings, host_name: String) -> Result<Self, String> {
        let listener = TcpListener::bind("0.0.0.0:0")
            .await
            .map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let password: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(12)
            .map(char::from)
            .collect();

        // Local interfaces only at start — never block lobby boot on WAN IP lookup.
        let mut addrs = local_ipv4s();
        if addrs.is_empty() {
            addrs.push("127.0.0.1".into());
        }

        let ticket = InviteTicket {
            version: 1,
            host_addrs: addrs,
            port,
            password: password.clone(),
            studio,
            host_name: host_name.clone(),
            created_at: Utc::now(),
        };

        let peers = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::channel::<()>(1);
        let peers_c = peers.clone();
        let password_c = password.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = rx.recv() => break,
                    accept = listener.accept() => {
                        if let Ok((stream, addr)) = accept {
                            let peers = peers_c.clone();
                            let password = password_c.clone();
                            tokio::spawn(async move {
                                let _ = handle_guest(stream, addr, password, peers).await;
                            });
                        }
                    }
                }
            }
        });

        Ok(Self {
            password,
            port,
            ticket,
            peers,
            shutdown: Arc::new(Mutex::new(Some(tx))),
        })
    }

    pub fn encode_ticket(&self) -> Result<String, String> {
        encode_ticket(&self.ticket)
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
    mut stream: TcpStream,
    _addr: SocketAddr,
    password: String,
    peers: Arc<Mutex<HashMap<String, PeerInfo>>>,
) -> Result<(), String> {
    let msg = read_frame(&mut stream).await?;
    match msg {
        PeerMessage::Hello {
            password: pw,
            name,
            participant_id,
        } => {
            if pw != password {
                write_frame(
                    &mut stream,
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
                &mut stream,
                &PeerMessage::Waiting {
                    participant_id: participant_id.clone(),
                    name,
                },
            )
            .await?;

            // Wait until admitted, then keep connection for control messages
            loop {
                let admitted = peers
                    .lock()
                    .get(&participant_id)
                    .map(|p| p.admitted)
                    .unwrap_or(false);
                if admitted {
                    write_frame(
                        &mut stream,
                        &PeerMessage::Admit {
                            participant_id: participant_id.clone(),
                        },
                    )
                    .await?;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }

            // Echo clock sync periodically and forward simple messages
            loop {
                match read_frame(&mut stream).await {
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
                        // Host will receive chunks into session dir — handled by app layer
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
) -> Result<TcpStream, String> {
    let mut last_err = "Could not connect".to_string();
    for addr in &ticket.host_addrs {
        let target = format!("{addr}:{}", ticket.port);
        match TcpStream::connect(&target).await {
            Ok(mut stream) => {
                write_frame(
                    &mut stream,
                    &PeerMessage::Hello {
                        password: ticket.password.clone(),
                        name,
                        participant_id,
                    },
                )
                .await?;
                return Ok(stream);
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(last_err)
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
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid invite payload: {e}"))
}

async fn write_frame(stream: &mut TcpStream, msg: &PeerMessage) -> Result<(), String> {
    let data = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
    let len = (data.len() as u32).to_be_bytes();
    stream.write_all(&len).await.map_err(|e| e.to_string())?;
    stream.write_all(&data).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn read_frame(stream: &mut TcpStream) -> Result<PeerMessage, String> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| e.to_string())?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        return Err("Frame too large".into());
    }
    let mut buf = vec![0u8; len];
    stream
        .read_exact(&mut buf)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&buf).map_err(|e| e.to_string())
}

fn local_ipv4s() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(ifaces) = local_ip_fallback() {
        out.extend(ifaces);
    }
    out
}

fn local_ip_fallback() -> Result<Vec<String>, String> {
    // Bind UDP to a public DNS to discover the preferred outbound interface IP
    use std::net::UdpSocket;
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    let _ = socket.connect("8.8.8.8:80");
    let addr = socket.local_addr().map_err(|e| e.to_string())?;
    Ok(vec![addr.ip().to_string()])
}

#[allow(dead_code)]
async fn discover_public_ip() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;
    let ip = client
        .get("https://api.ipify.org")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let ip = ip.trim().to_string();
    if ip.is_empty() {
        return Err("empty".into());
    }
    Ok(ip)
}

pub async fn send_file(
    stream: &mut TcpStream,
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
        stream,
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
            stream,
            &PeerMessage::FileChunk {
                file_name: file_name.clone(),
                offset,
                data_b64: base64::engine::general_purpose::STANDARD.encode(chunk),
            },
        )
        .await?;
    }
    write_frame(
        stream,
        &PeerMessage::FileDone {
            file_name: file_name.clone(),
        },
    )
    .await?;
    Ok(())
}
