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
use tokio::sync::mpsc;
use tokio_tungstenite::accept_async;
use tungstenite as ws;

pub type SessionQueue = deadqueue::limited::Queue<Session>;

pub type Message = String;

pub struct Session {
    id: u32,
    sender: mpsc::Sender<Message>,
}

#[allow(dead_code)]
impl Session {
    pub fn new(id: u32, sender: mpsc::Sender<Message>) -> Session {
        Session { id, sender }
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn send(&self, msg: Message) -> Result<()> {
        self.sender.blocking_send(msg)?;

        Ok(())
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
        info!("Acceptor listening on {}", self.addr);
        let listener = TcpListener::bind(&self.addr).await?;

        while let Ok((stream, _)) = listener.accept().await {
            // disable nagle's algorithm
            stream.set_nodelay(true).expect("Failed to set TCP_NODELAY");
            let peer = stream
                .peer_addr()
                .expect("connected streams should have a peer address");
            info!("Peer address: {}", peer);

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
        info!("New incoming connection on peer address {}", peer);
        if let Err(e) = Acceptor::handle(peer, stream, connections.clone(), id).await {
            use tungstenite::Error;
            match e {
                Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
                err => error!("Error processing connection: {}", err),
            }
        }
    }

    async fn handle(
        peer: SocketAddr,
        stream: TcpStream,
        connections: Arc<SessionQueue>,
        id: Arc<AtomicU32>,
    ) -> tungstenite::Result<()> {
        let ws_stream = accept_async(stream).await.expect("Failed to accept");
        info!("Connection established with peer address {}", peer);
        let (mut ws_sender, mut ws_receiver) = ws_stream.split();
        let (tx, mut rx) = mpsc::channel::<String>(1);
        let id = id.fetch_add(1, Ordering::SeqCst);
        let _ = connections.push(Session::new(id, tx)).await;

        loop {
            if util::Control::should_stop() {
                break;
            }
            tokio::select! {
                Some(msg) = ws_receiver.next() => {
                    let msg = msg?;
                    if msg.is_text() || msg.is_binary() {
                        ws_sender.send(msg).await?;
                    } else if msg.is_close() {
                        break;
                    }
                },
                Some(msg) = rx.recv() => {
                    ws_sender.send(ws::Message::Text(msg)).await?;
                }
            }
        }

        Ok(())
    }
}
