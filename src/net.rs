use crate::util;
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use std::thread;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;
use tungstenite as ws;

pub type SessionQueue = deadqueue::limited::Queue<Session>;
pub type Message = String;
pub type MessageQueue = deadqueue::limited::Queue<Message>;

pub struct Session {
    id: u32,
    queue: Arc<MessageQueue>,
}

#[allow(dead_code)]
impl Session {
    pub fn new(id: u32, queue: Arc<MessageQueue>) -> Session {
        Session { id, queue }
    }

    pub fn id(&self) -> u32 {
        self.id
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
        info!(target: "Acceptor", "Listening on {}", self.addr);
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
        info!(target: "Acceptor", "New peer {}", peer);
        if let Err(e) = Socket::start(peer, stream, connections.clone(), id).await {
            handle_socket_error(e);
        }
    }
}

fn handle_socket_error(err: tungstenite::Error) {
    use tungstenite::Error;
    match err {
        Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
        err => error!("Error processing connection: {}", err),
    }
}

struct Socket;
impl Socket {
    async fn start(
        peer: SocketAddr,
        stream: TcpStream,
        connections: Arc<SessionQueue>,
        id: Arc<AtomicU32>,
    ) -> tungstenite::Result<()> {
        let mut ws = accept_async(stream).await.expect("Failed to accept");
        info!(target: "WebSocket", "Connection established with peer {}", peer);

        // TODO: what's a good size for the message queue?
        let queue = Arc::new(MessageQueue::new(4));
        connections
            .push(Session::new(
                id.fetch_add(1, Ordering::SeqCst),
                queue.clone(),
            ))
            .await;

        loop {
            if util::Control::should_stop() {
                break;
            }
            tokio::select! {
                Some(msg) = ws.next() => {
                    let msg = msg?;
                    if msg.is_text() || msg.is_binary() {
                        ws.send(msg).await?;
                    } else if msg.is_close() {
                        break;
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
