extern crate futures_util;
extern crate tokio;
#[macro_use]
extern crate log;
extern crate ctrlc;
extern crate deadqueue;
extern crate dotenv;
extern crate pretty_env_logger;
extern crate tokio_tungstenite;
extern crate tungstenite;

use futures_util::{SinkExt, StreamExt};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};
use std::thread::spawn;
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::accept_async;
use tungstenite::Message;

type SessionQueue = deadqueue::limited::Queue<Session>;

struct Session {
    pub id: u32,
    pub sender: mpsc::Sender<String>,
}

async fn accept_connection(
    peer: SocketAddr,
    stream: TcpStream,
    connections: Arc<SessionQueue>,
    id: Arc<AtomicU32>,
    shutdown: Arc<AtomicBool>,
) {
    if let Err(e) = handle_connection(peer, stream, connections.clone(), id, shutdown).await {
        use tungstenite::Error;
        match e {
            Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
            err => error!("Error processing connection: {}", err),
        }
    }
}

async fn handle_connection(
    peer: SocketAddr,
    stream: TcpStream,
    connections: Arc<SessionQueue>,
    id: Arc<AtomicU32>,
    shutdown: Arc<AtomicBool>,
) -> tungstenite::Result<()> {
    let ws_stream = accept_async(stream).await.expect("Failed to accept");
    info!("New WebSocket connection: {}", peer);
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let (tx, mut rx) = mpsc::channel::<String>(1);
    let id = id.fetch_add(1, Ordering::SeqCst);
    let _ = connections.push(Session { id, sender: tx }).await;

    loop {
        if shutdown.load(Ordering::SeqCst) {
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
                info!("Sending text '{}' to session #{}", msg, id);
                ws_sender.send(Message::Text(msg)).await?;
            }
        }
    }

    Ok(())
}

fn listener(connections: Arc<SessionQueue>, shutdown: Arc<AtomicBool>) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime")
        .block_on(async {
            let addr = "127.0.0.1:9002";
            let listener = TcpListener::bind(&addr).await.expect("Can't listen");
            info!("Listening on: {}", addr);
            let id = Arc::new(AtomicU32::new(0));

            while let Ok((stream, _)) = listener.accept().await {
                // disable nagle's algorithm
                stream.set_nodelay(true).expect("Failed to set TCP_NODELAY");
                let peer = stream
                    .peer_addr()
                    .expect("connected streams should have a peer address");
                info!("Peer address: {}", peer);

                tokio::spawn(accept_connection(
                    peer,
                    stream,
                    connections.clone(),
                    id.clone(),
                    shutdown.clone(),
                ));
            }
        });
}

fn tick(sessions: &mut Vec<Session>) {
    info!("Tick");
    for session in sessions {
        session
            .sender
            .blocking_send("tick".into())
            .expect("Failed to send message");
    }
}

fn main() {
    dotenv::dotenv().unwrap();
    pretty_env_logger::init();

    let shutdown = Arc::new(AtomicBool::new(false));
    let handler_shutdown = shutdown.clone();
    ctrlc::set_handler(move || {
        handler_shutdown.store(true, Ordering::SeqCst);
    })
    .expect("Failed to setup SIGINT/SIGTERM handler");

    let queue = Arc::new(SessionQueue::new(10));

    let listener_queue = queue.clone();
    let listener_shutdown = shutdown.clone();
    spawn(move || {
        listener(listener_queue, listener_shutdown);
    });

    let mut sessions = Vec::<Session>::new();
    let mut then = Instant::now();
    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        while let Some(session) = queue.try_pop() {
            sessions.push(session);
        }

        let now = Instant::now();
        if now - then >= Duration::from_secs(1) {
            then = now;

            tick(&mut sessions);
        }
    }
}
