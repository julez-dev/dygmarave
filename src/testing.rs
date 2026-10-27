//! A scripted in-memory keyboard, available to tests only.
//!
//! Tests use it to assert on session behaviour (baseline capture, restore order,
//! frame size discipline) without a port. It can also be told to start failing,
//! which is how the benchmark's error path is covered.

use std::collections::HashMap;

use crate::link::{Link, LinkError};

#[derive(Default)]
pub struct FakeLink {
    /// Every command received, in order, exactly as the caller wrote it.
    pub log: Vec<String>,
    /// Canned replies keyed by command.
    pub replies: HashMap<String, String>,
    /// After this many commands, every call fails. `None` means never fail.
    pub fail_after: Option<usize>,
    /// How long each command pretends to take.
    pub delay: Option<std::time::Duration>,
    /// Acknowledgements the fake is pretending are waiting to be discarded.
    pub pending_acks: usize,
}

impl FakeLink {
    pub fn with_replies(pairs: &[(&str, &str)]) -> Self {
        Self {
            replies: pairs
                .iter()
                .map(|(command, reply)| (command.to_string(), reply.to_string()))
                .collect(),
            ..Self::default()
        }
    }

    /// Counts how many commands sent began with `prefix`.
    pub fn count(&self, prefix: &str) -> usize {
        self.log
            .iter()
            .filter(|command| command.starts_with(prefix))
            .count()
    }

    fn check(&self) -> Result<(), LinkError> {
        if let Some(limit) = self.fail_after {
            if self.log.len() >= limit {
                return Err(LinkError::Io("simulated dead port".to_string()));
            }
        }
        Ok(())
    }
}

impl Link for FakeLink {
    fn send(&mut self, command: &str) -> Result<(), LinkError> {
        self.check()?;
        self.log.push(command.to_string());
        if let Some(delay) = self.delay {
            std::thread::sleep(delay);
        }
        Ok(())
    }

    fn drain(&mut self) -> Result<usize, LinkError> {
        self.check()?;
        let drained = self.pending_acks;
        self.pending_acks = 0;
        Ok(drained)
    }

    fn query(&mut self, command: &str) -> Result<String, LinkError> {
        self.send(command)?;
        Ok(self.replies.get(command).cloned().unwrap_or_default())
    }
}
