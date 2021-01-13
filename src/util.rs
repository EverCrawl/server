use anyhow::Result;
use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};

static CONTROL: AtomicBool = AtomicBool::new(false);
#[derive(Clone)]
pub struct Control {}
#[allow(dead_code)]
impl Control {
    pub fn init() -> Result<()> {
        ctrlc::set_handler(|| {
            CONTROL.store(true, Ordering::SeqCst);
        })?;

        Ok(())
    }

    pub fn should_stop() -> bool {
        CONTROL.load(Ordering::SeqCst)
    }

    pub fn should_stop_async() -> ShouldStop {
        ShouldStop
    }

    pub fn stop() {
        CONTROL.store(true, Ordering::SeqCst);
    }
}

pub struct ShouldStop;
impl Future for ShouldStop {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        if Control::should_stop() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}
