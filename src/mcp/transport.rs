//! Budgets applied before SDK buffering/dispatch, including clients that send
//! oversized frames or stop reading responses. No additional transport.
use rmcp::{
    RoleServer,
    model::{ClientJsonRpcMessage, RequestId, ServerJsonRpcMessage},
    transport::{Transport, async_rw::AsyncRwTransport},
};
use std::{
    collections::HashSet,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

const MAX_FRAME: usize = 64 * 1024;
const MAX_IN_FLIGHT: usize = 16;

struct LineLimit<R> {
    inner: R,
    used: usize,
}
impl<R: AsyncRead + Unpin> AsyncRead for LineLimit<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if out.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut bytes = [0; 8192];
        let size = bytes.len().min(out.remaining());
        let mut buffer = ReadBuf::new(&mut bytes[..size]);
        match Pin::new(&mut self.inner).poll_read(cx, &mut buffer) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        for byte in buffer.filled() {
            if *byte == b'\n' {
                self.used = 0;
            } else {
                self.used += 1;
            }
            if self.used > MAX_FRAME {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "MCP frame exceeds 64 KiB",
                )));
            }
        }
        out.put_slice(buffer.filled());
        Poll::Ready(Ok(()))
    }
}

struct Bounded<T> {
    inner: T,
    pending: Arc<Mutex<HashSet<RequestId>>>,
}
impl<T: Transport<RoleServer, Error = io::Error>> Transport<RoleServer> for Bounded<T> {
    type Error = io::Error;
    fn send(
        &mut self,
        item: ServerJsonRpcMessage,
    ) -> impl Future<Output = io::Result<()>> + Send + 'static {
        let id = match &item {
            ServerJsonRpcMessage::Response(r) => Some(r.id.clone()),
            ServerJsonRpcMessage::Error(e) => e.id.clone(),
            _ => None,
        };
        let pending = self.pending.clone();
        let sent = self.inner.send(item);
        async move {
            let result = sent.await;
            // Release only when bytes have been written, not merely queued.
            if let Some(id) = id {
                pending.lock().unwrap().remove(&id);
            }
            result
        }
    }
    async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
        let item = self.inner.receive().await?;
        if let ClientJsonRpcMessage::Request(r) = &item {
            let mut pending = self.pending.lock().unwrap();
            if pending.len() >= MAX_IN_FLIGHT || !pending.insert(r.id.clone()) {
                eprintln!(
                    "agentrun-mcp: too many outstanding requests or duplicate request ID; closing stdio session"
                );
                return None;
            }
        }
        Some(item)
    }
    async fn close(&mut self) -> io::Result<()> {
        self.inner.close().await
    }
}
pub fn bounded_stdio() -> impl Transport<RoleServer, Error = io::Error> {
    Bounded {
        inner: AsyncRwTransport::new_server(
            LineLimit {
                inner: tokio::io::stdin(),
                used: 0,
            },
            tokio::io::stdout(),
        ),
        pending: Arc::new(Mutex::new(HashSet::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn frame_budget_counts_bytes_across_reads_and_resets_at_newlines() {
        let bytes = vec![b'x'; MAX_FRAME + 1];
        let mut reader = LineLimit {
            inner: bytes.as_slice(),
            used: 0,
        };
        let mut out = Vec::new();
        assert_eq!(
            reader.read_to_end(&mut out).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(out.len() <= MAX_FRAME);
        let mut bytes = vec![b'x'; MAX_FRAME];
        bytes.push(b'\n');
        bytes.extend_from_slice(&vec![b'y'; MAX_FRAME]);
        let mut reader = LineLimit {
            inner: bytes.as_slice(),
            used: 0,
        };
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, bytes);
    }
    struct TestTransport(VecDeque<ClientJsonRpcMessage>);
    impl Transport<RoleServer> for TestTransport {
        type Error = io::Error;
        fn send(
            &mut self,
            _: ServerJsonRpcMessage,
        ) -> impl Future<Output = io::Result<()>> + Send + 'static {
            std::future::ready(Ok(()))
        }
        async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
            self.0.pop_front()
        }
        async fn close(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn unread_responses_hold_request_budget_until_written() {
        let requests = (0..=MAX_IN_FLIGHT)
            .map(|id| {
                serde_json::from_value(serde_json::json!({"jsonrpc":"2.0","id":id,"method":"ping"}))
                    .unwrap()
            })
            .collect();
        let mut transport = Bounded {
            inner: TestTransport(requests),
            pending: Arc::new(Mutex::new(HashSet::new())),
        };
        for _ in 0..MAX_IN_FLIGHT {
            assert!(transport.receive().await.is_some());
        }
        // Constructing a send future must not release capacity before it runs.
        let send = transport.send(
            serde_json::from_value(serde_json::json!({"jsonrpc":"2.0","id":0,"result":{}}))
                .unwrap(),
        );
        assert_eq!(transport.pending.lock().unwrap().len(), MAX_IN_FLIGHT);
        send.await.unwrap();
        assert!(transport.receive().await.is_some());
        transport.inner.0.push_back(
            serde_json::from_value(serde_json::json!({"jsonrpc":"2.0","id":99,"method":"ping"}))
                .unwrap(),
        );
        assert!(transport.receive().await.is_none());
        assert_eq!(transport.pending.lock().unwrap().len(), MAX_IN_FLIGHT);
    }
}
