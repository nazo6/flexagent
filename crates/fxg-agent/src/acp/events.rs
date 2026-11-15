//! Event forwarding for the ACP driver.
//!
//! [`EventSink`] forwards [`DriverEvent`]s to the session manager after
//! applying the extension's synchronous [`AgentExtension::transform_event`].
//! Keeping it synchronous avoids the extra relay task and the event clone the
//! old `ExtendedAcpDriver` needed.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::driver::DriverEvent;
use crate::extension::{AgentExtension, EventDisposition};

/// Sink that forwards driver events through an optional extension transform.
#[derive(Clone)]
pub(super) struct EventSink {
    tx: mpsc::UnboundedSender<DriverEvent>,
    extension: Option<Arc<dyn AgentExtension>>,
}

impl EventSink {
    /// Creates a sink that applies `extension` to every event.
    pub(super) fn new(
        tx: mpsc::UnboundedSender<DriverEvent>,
        extension: Option<Arc<dyn AgentExtension>>,
    ) -> Self {
        Self { tx, extension }
    }

    /// Sends one event (or its transformed replacement(s)).
    pub(super) fn send(
        &self,
        event: DriverEvent,
    ) -> Result<(), Box<mpsc::error::SendError<DriverEvent>>> {
        match &self.extension {
            None => self.forward(event),
            Some(extension) => match extension.transform_event(event) {
                EventDisposition::Emit(event) => self.forward(*event),
                EventDisposition::EmitMany(events) => {
                    for event in events {
                        self.forward(event)?;
                    }
                    Ok(())
                }
                EventDisposition::Drop => Ok(()),
            },
        }
    }

    /// Forwards one event to the session manager.
    fn forward(&self, event: DriverEvent) -> Result<(), Box<mpsc::error::SendError<DriverEvent>>> {
        self.tx.send(event).map_err(Box::new)
    }
}
