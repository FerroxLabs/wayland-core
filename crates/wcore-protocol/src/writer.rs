use std::io;
use std::sync::Arc;

use crate::events::ProtocolEvent;
use crate::output_pump::{OutputPump, OutputStream};

/// Trait for emitting protocol events to a host.
///
/// The default implementation (`ProtocolWriter`) writes JSON Lines to stdout.
/// Backend integrations provide alternative implementations that bridge events
/// to their own event systems.
pub trait ProtocolEmitter: Send + Sync {
    fn emit(&self, event: &ProtocolEvent) -> io::Result<()>;
}

/// Thread-safe JSON Lines writer to stdout
pub struct ProtocolWriter {
    output: OutputPump,
}

impl Default for ProtocolWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolWriter {
    pub fn new() -> Self {
        Self::new_with_failure_handler(Arc::new(|| {
            tracing::error!("protocol output delivery failed");
        }))
    }

    pub fn new_with_failure_handler(handler: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            output: OutputPump::new_with_failure_handler(handler),
        }
    }

    /// Wait at most 100 ms for every event accepted before this call to reach
    /// process stdout. The writer remains open on success; timeout and late
    /// output failures are returned to the caller.
    pub fn flush_bounded(&self) -> io::Result<()> {
        self.output.flush_bounded()
    }

    #[cfg(test)]
    fn with_writer<F>(writer: F) -> Self
    where
        F: Fn(OutputStream, &[u8]) -> io::Result<()> + Send + 'static,
    {
        Self {
            output: OutputPump::with_writer(writer, None),
        }
    }
}

impl ProtocolEmitter for ProtocolWriter {
    fn emit(&self, event: &ProtocolEvent) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(event)
            .map_err(|e| io::Error::other(format!("failed to serialize protocol event: {e}")))?;
        bytes.push(b'\n');
        self.output.write(OutputStream::Stdout, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::events::ProtocolEvent;

    #[test]
    fn test_writer_construction() {
        let _writer = ProtocolWriter::new();
    }

    #[test]
    fn writer_delivers_exact_json_lines_in_order() {
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let delivered_for_writer = Arc::clone(&delivered);
        let writer = ProtocolWriter::with_writer(move |stream, bytes| {
            assert_eq!(stream, OutputStream::Stdout);
            delivered_for_writer
                .lock()
                .unwrap()
                .extend_from_slice(bytes);
            Ok(())
        });

        writer
            .emit(&ProtocolEvent::StreamStart {
                msg_id: "message-1".to_string(),
            })
            .unwrap();
        writer
            .emit(&ProtocolEvent::TextDelta {
                text: "hello".to_string(),
                msg_id: "message-1".to_string(),
            })
            .unwrap();
        writer.flush_bounded().unwrap();

        assert_eq!(
            delivered.lock().unwrap().as_slice(),
            b"{\"type\":\"stream_start\",\"msg_id\":\"message-1\"}\n\
              {\"type\":\"text_delta\",\"text\":\"hello\",\"msg_id\":\"message-1\"}\n"
        );
    }
}
