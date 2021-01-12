use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
pub struct Control {}

static CONTROL: AtomicBool = AtomicBool::new(false);

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

    pub fn stop() {
        CONTROL.store(true, Ordering::SeqCst);
    }
}
