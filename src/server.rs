use crate::net;
use crate::util;
use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Server {
    acceptor: Arc<net::Acceptor>,
    // TODO: move into separate struct Game
    session_queue: Arc<net::SessionQueue>,
    sessions: Vec<net::Session>,
}

impl Server {
    pub fn new(addr: &str) -> Server {
        let session_queue = Arc::new(net::SessionQueue::new(4));

        Server {
            acceptor: Arc::new(net::Acceptor::new(addr, session_queue.clone())),
            session_queue,
            sessions: Vec::new(),
        }
    }

    pub fn start(&mut self) -> Result<()> {
        info!(target: "Server", "Starting...");
        let acceptor = self.acceptor.clone();
        net::spawn_network(async move {
            if let Err(e) = acceptor.start().await {
                panic!(e);
            }
        })?;
        info!(target: "Server", "Startup successful");

        let mut then = Instant::now();
        loop {
            if util::Control::should_stop() {
                break;
            }

            let now = Instant::now();
            if now - then >= Duration::from_secs(1) {
                then = now;

                self.tick();
            }
        }

        Ok(())
    }

    fn tick(&mut self) {
        info!(target: "Server", "Tick");
        // handle new sessions
        while let Some(session) = self.session_queue.try_pop() {
            self.sessions.push(session);
        }

        for session in self.sessions.iter_mut() {
            while let Some(msg) = session.recv() {
                // echo all messages back
                session.send(msg);
            }
        }

        for session in self.sessions.iter() {
            session.send("Tick".into());
        }
    }
}
