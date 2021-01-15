use crate::config::Config;
use crate::db;
use crate::net;
use crate::unrecoverable;
use crate::util;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tokio_compat_02::FutureExt;

/// Represents a game server instance, encapsulating all of its functionality.
pub struct Server {
    acceptor: Arc<net::Acceptor>,
    database: Arc<db::Database>,
    // TODO: move into separate struct Game
    squeue: Arc<net::SessionQueue>,
    sessions: HashMap<u32, net::Session>,
}

impl Server {
    pub fn new(config: Config) -> Server {
        let queue = Arc::new(net::SessionQueue::new(4));

        Server {
            acceptor: Arc::new(net::Acceptor::new(
                config.server.address.clone(),
                queue.clone(),
            )),
            database: Arc::new(db::Database::new(config.database, 4)),
            squeue: queue,
            sessions: HashMap::new(),
        }
    }

    pub fn start(&mut self) -> Result<()> {
        log::info!(target: "Server", "Starting...");

        let acceptor = self.acceptor.clone();
        let db = self.database.clone();
        let runtime = tokio::runtime::Runtime::new()?;
        thread::spawn(move || {
            runtime.block_on(async move {
                // TODO: remove .compat() after sqlx tokio updates to 1.0
                unrecoverable!(db.start().compat().await);
                unrecoverable!(acceptor.start().await);
            });
        });
        log::info!(target: "Server", "Startup successful");

        let mut then = Instant::now();
        loop {
            if util::Control::should_stop() {
                return Ok(());
            }

            let now = Instant::now();
            if now - then >= Duration::from_secs_f64(1.0 / 30.0) {
                then = now;

                self.tick();
            }
        }
    }

    fn tick(&mut self) {
        // info!(target: "Server", "Tick");
        // handle new sessions
        while let Some(event) = self.squeue.try_pop() {
            use net::ClientEvent;
            match event {
                ClientEvent::Connected(session) => {
                    log::info!(target: "Server", "Server got client {}", session.id());
                    self.sessions.insert(session.id(), session)
                }
                ClientEvent::Disconnected(id) => {
                    log::info!(target: "Server", "Server lost client {}", id);
                    self.sessions.remove(&id)
                }
            };
        }

        for (_, session) in self.sessions.iter_mut() {
            while let Some(msg) = session.recv() {
                // echo all messages back
                session.send(msg);
            }
        }

        for (_, session) in self.sessions.iter() {
            session.send("Tick".into());
        }
    }
}
