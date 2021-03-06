use crate::util::Control;
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite as ws;
use ws::WebSocketStream;

pub struct InvalidCredentials {
    pub token: String,
}

impl InvalidCredentials {
    pub async fn validate(self) -> Result<Credentials> {
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

pub async fn wait_for_credentials(ws: &mut WebSocketStream<TcpStream>) -> Result<Credentials> {
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

pub async fn authenticate(stream: TcpStream) -> Result<(WebSocketStream<TcpStream>, Credentials)> {
    let mut ws = ws::accept_async(stream).await?;
    let credentials = wait_for_credentials(&mut ws).await?;
    Ok((ws, credentials))
}
