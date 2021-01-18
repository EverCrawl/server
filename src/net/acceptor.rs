use super::auth::authenticate;
use super::session;
use super::socket::Socket;
use std::sync::atomic::Ordering;
use std::{
    net::SocketAddr,
    sync::{atomic::AtomicU32, Arc},
};
use tokio::net::{TcpListener, TcpStream};

pub struct Acceptor {
    addr: String,
    id: Arc<AtomicU32>,
    squeue: Arc<session::Queue>,
}

impl Acceptor {
    pub fn new(addr: String, squeue: Arc<session::Queue>) -> Acceptor {
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
        squeue: Arc<session::Queue>,
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
