use crate::util::Control;
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::convert::From;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, WebSocketStream};
use tungstenite as ws;

pub type SessionQueue = deadqueue::limited::Queue<ClientEvent>;
pub type Message = String;
pub type MessageQueue = deadqueue::limited::Queue<Message>;

// TODO: anytime the client does anything unexpected, immediately disconnect them

pub enum ClientEvent {
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

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn cred(&self) -> &Credentials {
        &self.credentials
    }

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

    pub fn recv(&mut self) -> Option<Message> {
        self.queue.try_pop()
    }
}

pub fn spawn_network<F: std::future::Future + Sync + Send + 'static>(
    future: F,
) -> Result<thread::JoinHandle<()>> {
    // Runtime is built before spawning the thread, so the error can immediately propagate
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    Ok(thread::spawn(move || {
        runtime.block_on(future);
    }))
}

pub struct Acceptor {
    addr: String,
    id: Arc<AtomicU32>,
    connections: Arc<SessionQueue>,
}

impl Acceptor {
    pub fn new(addr: &str, connections: Arc<SessionQueue>) -> Acceptor {
        Acceptor {
            addr: addr.to_string(),
            id: Arc::new(AtomicU32::new(0)),
            connections,
        }
    }

    pub async fn start(&self) -> Result<(), tokio::io::Error> {
        log::info!(target: "Acceptor", "Listening on {}", self.addr);
        let listener = TcpListener::bind(&self.addr).await?;

        while let Ok((stream, _)) = listener.accept().await {
            // disable nagle's algorithm
            stream.set_nodelay(true).expect("Failed to set TCP_NODELAY");
            let peer = stream
                .peer_addr()
                .expect("connected streams should have a peer address");

            tokio::spawn(Acceptor::accept(
                peer,
                stream,
                self.connections.clone(),
                self.id.clone(),
            ));
        }

        Ok(())
    }

    async fn accept(
        peer: SocketAddr,
        stream: TcpStream,
        connections: Arc<SessionQueue>,
        id: Arc<AtomicU32>,
    ) {
        log::info!(target: "Acceptor", "New peer {}", peer);
        if let Err(err) = Socket::start(
            peer,
            stream,
            connections.clone(),
            id.fetch_add(1, Ordering::SeqCst),
        )
        .await
        {
            use tungstenite::Error;
            match err.downcast::<Error>() {
                Ok(err) => match err {
                    Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
                    err => log::error!("Error processing connection: {}", err),
                },
                Err(err) => log::error!("Error processing connection: {}", err),
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
        log::info!(target: "Authentication", "Token: {}", self.token);
        Ok(Credentials { token: self.token })
    }
}
impl From<String> for InvalidCredentials {
    fn from(token: String) -> Self {
        InvalidCredentials { token }
    }
}

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

struct Socket;
impl Socket {
    async fn start(
        peer: SocketAddr,
        stream: TcpStream,
        connections: Arc<SessionQueue>,
        id: u32,
    ) -> Result<()> {
        let (mut ws, cred) = authenticate(stream).await?;
        log::info!(target: "WebSocket", "Connection established with peer {}", peer);

        // TODO: what's a good size for the message queue?
        let queue = Arc::new(MessageQueue::new(4));
        connections
            .push(ClientEvent::Connected(Session::new(
                id,
                cred,
                queue.clone(),
            )))
            .await;

        loop {
            tokio::select! {
                _ = Control::should_stop_async() => {
                    // Don't need to notify server about disconnection
                    // Everyone is getting disconnected, process shutdown
                    // is our garbage collector :)
                    break;
                }
                Some(msg) = ws.next() => {
                    let msg = msg?;
                    // TODO: binary only instead of text
                    if msg.is_text() {
                        queue.push(msg.into_text().unwrap()).await;
                    } else if msg.is_close() {
                        connections.push(ClientEvent::Disconnected(id)).await;
                    }
                },
                msg = queue.pop() => {
                    ws.send(ws::Message::Text(msg)).await?;
                }
            }
        }

        Ok(())
    }
}
