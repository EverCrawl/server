use crate::util::Control;
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::convert::From;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, WebSocketStream};
use tungstenite as ws;

pub type SessionQueue = deadqueue::limited::Queue<SessionEvent>;
pub type Message = String;
pub type MessageQueue = deadqueue::limited::Queue<Message>;

// TODO: anytime the client does anything unexpected, immediately disconnect them
// TODO: figure out how to prevent clients from spamming

pub enum SessionEvent {
    Connected(Session),
    Disconnected(u32),
}

pub struct Session {
    id: u32,
    credentials: Credentials,
    queue: Arc<MessageQueue>,
}

#[allow(dead_code)]
impl Session {
    pub fn new(id: u32, credentials: Credentials, queue: Arc<MessageQueue>) -> Session {
        Session {
            id,
            credentials,
            queue,
        }
    }

    #[inline]
    pub fn id(&self) -> u32 {
        self.id
    }

    #[inline]
    pub fn cred(&self) -> &Credentials {
        &self.credentials
    }

    #[inline]
    pub fn send(&self, mut msg: Message) {
        // If the queue is full, keep trying to send the message.
        loop {
            // try_push returns the msg as Err(msg) if the queue is full
            msg = match self.queue.try_push(msg) {
                Err(e) => e,
                Ok(()) => return,
            }
        }
    }

    #[inline]
    pub fn recv(&mut self) -> Option<Message> {
        self.queue.try_pop()
    }
}

pub struct Acceptor {
    addr: String,
    id: Arc<AtomicU32>,
    squeue: Arc<SessionQueue>,
}

impl Acceptor {
    pub fn new(addr: String, squeue: Arc<SessionQueue>) -> Acceptor {
        Acceptor {
            addr,
            id: Arc::new(AtomicU32::new(0)),
            squeue,
        }
    }

    pub async fn start(self: Arc<Self>) -> Result<(), tokio::io::Error> {
        let listener = TcpListener::bind(&self.addr).await?;
        log::info!(target: "Acceptor", "Bound to {}", self.addr);

        while let Ok((stream, _)) = listener.accept().await {
            // disable nagle's algorithm
            stream.set_nodelay(true).expect("Failed to set TCP_NODELAY");
            let peer = stream
                .peer_addr()
                .expect("connected streams should have a peer address");

            tokio::spawn(Acceptor::accept(
                peer,
                stream,
                self.squeue.clone(),
                self.id.clone(),
            ));
        }

        Ok(())
    }

    async fn accept(
        peer: SocketAddr,
        stream: TcpStream,
        squeue: Arc<SessionQueue>,
        id: Arc<AtomicU32>,
    ) {
        log::debug!(target: "Acceptor", "New peer {}, authenticating", peer);
        let (ws, cred) = match authenticate(stream).await {
            Ok(value) => value,
            Err(err) => return log::error!(target: "Socket", "{}", err),
        };
        log::debug!(target: "Acceptor", "Peer {} authenticated", peer);

        if let Err(err) = Arc::new(Socket::new(id.fetch_add(1, Ordering::SeqCst), peer, cred))
            .start(ws, squeue)
            .await
        {
            use tungstenite::Error;
            match err.downcast::<tungstenite::Error>() {
                Ok(err) => match err {
                    Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
                    err => log::error!(target: "Socket", "{}", err),
                },
                Err(err) => log::error!(target: "Socket", "{}", err),
            }
        }
    }
}

struct InvalidCredentials {
    pub token: String,
}

impl InvalidCredentials {
    async fn validate(self) -> Result<Credentials> {
        // TODO: query database here
        log::trace!(target: "Authentication", "Token: {}", self.token);
        Ok(Credentials { token: self.token })
    }
}
impl From<String> for InvalidCredentials {
    fn from(token: String) -> Self {
        InvalidCredentials { token }
    }
}

#[derive(Clone)]
pub struct Credentials {
    pub token: String,
}

async fn wait_for_credentials(ws: &mut WebSocketStream<TcpStream>) -> Result<Credentials> {
    tokio::select! {
        // Either:
        // Server stops
        _ = Control::should_stop_async() => {
            Err(anyhow::anyhow!("Failed to authenticate"))
        }
        // Authentication times out
        _ = tokio::time::sleep(Duration::from_secs(1)) => {
            Err(anyhow::anyhow!("Failed to authenticate"))
        }
        // We receive the authentication message
        Some(msg) = ws.next() => {
            let msg = msg?;
            if msg.is_text() {
                // If so, it gets validated by querying the DB
                Ok(InvalidCredentials::from(msg.into_text()?).validate().await?)
            } else {
                let _ = ws.send("Failed to authenticate".into()).await;
                Err(anyhow::anyhow!("Failed to authenticate"))
            }
        }
    }
}

async fn authenticate(stream: TcpStream) -> Result<(WebSocketStream<TcpStream>, Credentials)> {
    let mut ws = accept_async(stream).await?;
    let credentials = wait_for_credentials(&mut ws).await?;
    Ok((ws, credentials))
}

struct Socket {
    id: u32,
    peer: SocketAddr,
    credentials: Credentials,
}
impl Socket {
    fn new(id: u32, peer: SocketAddr, credentials: Credentials) -> Socket {
        Socket {
            id,
            peer,
            credentials,
        }
    }

    async fn start(
        self: Arc<Self>,
        mut stream: WebSocketStream<TcpStream>,
        squeue: Arc<SessionQueue>,
    ) -> Result<()> {
        log::debug!(target: "WebSocket", "{} connected", self.peer);

        // TODO: what's a good size for the message queue?
        let mqueue = Arc::new(MessageQueue::new(4));

        squeue
            .push(SessionEvent::Connected(Session::new(
                self.id,
                self.credentials.clone(),
                mqueue.clone(),
            )))
            .await;

        loop {
            tokio::select! {
                _ = Control::should_stop_async() => {
                    break;
                }
                Some(msg) = stream.next() => {
                    let msg = msg?;
                    // TODO: binary only instead of text
                    if msg.is_text() {
                        mqueue.push(msg.into_text().unwrap()).await;
                    } else if msg.is_close() {
                        squeue.push(SessionEvent::Disconnected(self.id)).await;
                    }
                },
                msg = mqueue.pop() => {
                    stream.send(ws::Message::Text(msg)).await?;
                }
            }
        }

        Ok(())
    }
}
