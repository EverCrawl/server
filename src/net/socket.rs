use super::auth::Credentials;
use super::message;
use super::session;
use crate::util::Control;
use anyhow::Result;
use deadqueue::limited;
use futures_util::{SinkExt, StreamExt};
use message::Message;
use session::Session;
use std::{net::SocketAddr, sync::Arc};
use tokio::net::TcpStream;
use tokio_tungstenite as ws;
use ws::WebSocketStream;

pub type Sender = limited::Queue<Vec<u8>>;

pub struct Socket {
    id: u32,
    peer: SocketAddr,
    credentials: Credentials,
}
impl Socket {
    pub fn new(id: u32, peer: SocketAddr, credentials: Credentials) -> Socket {
        Socket {
            id,
            peer,
            credentials,
        }
    }

    pub async fn start(
        self: Arc<Self>,
        mut stream: WebSocketStream<TcpStream>,
        squeue: Arc<session::Queue>,
    ) -> Result<()> {
        log::debug!(target: "WebSocket", "{} connected", self.peer);

        // TODO: what's a good size for the message queues?
        // outgoing (server -> client)
        let out_queue = Arc::new(Sender::new(1));
        // incoming (client -> server)
        let in_queue = Arc::new(message::Queue::new(4));

        squeue
            .push(session::Event::Connected(Session::new(
                self.id,
                self.credentials.clone(),
                out_queue.clone(),
                in_queue.clone(),
            )))
            .await;

        loop {
            tokio::select! {
                // server shutdown
                _ = Control::should_stop_async() => {
                    break;
                }
                // client -> server
                Some(msg) = stream.next() => {
                    let msg = msg?;
                    // TODO: binary only instead of text
                    if msg.is_binary() {
                        in_queue.push(Message::parse(msg.into_data())?).await;
                    } else if msg.is_close() {
                        squeue.push(session::Event::Disconnected(self.id)).await;
                    }
                    // anything else is dropped
                    // NOTE: maybe disconnect, because it's unexpected?
                },
                // server -> client
                msg = out_queue.pop() => {
                    stream.send(tungstenite::Message::Binary(msg)).await?;
                }
            }
        }

        Ok(())
    }
}
