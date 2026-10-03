//! Cancellation belongs to a response, never to whichever recording starts next.
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tokio::sync::Notify;

pub const CANCELLED: &str = "Cancelled";

#[derive(Clone, Default)]
pub struct Cancellation(Arc<Signal>);

#[derive(Default)]
struct Signal {
    cancelled: AtomicBool,
    changed: Notify,
    delivery: Mutex<()>,
}

impl Cancellation {
    pub fn cancel(&self) {
        let _delivery = self.0.delivery.lock().unwrap();
        self.0.cancelled.store(true, Ordering::SeqCst);
        self.0.changed.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool { self.0.cancelled.load(Ordering::SeqCst) }

    pub fn check(&self) -> Result<(), String> {
        if self.is_cancelled() { Err(CANCELLED.into()) } else { Ok(()) }
    }

    pub async fn cancelled(&self) {
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_cancelled() { return; }
            changed.await;
        }
    }

    pub async fn run<F: Future>(&self, future: F) -> Result<F::Output, String> {
        tokio::select! {
            biased;
            _ = self.cancelled() => Err(CANCELLED.into()),
            result = future => Ok(result),
        }
    }

    /// Serialize the final paste shortcut with cancellation: whichever wins happens first.
    pub fn run_if_active<T>(&self, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let _delivery = self.0.delivery.lock().unwrap();
        self.check()?;
        action()
    }
}

#[derive(Default)]
pub struct Responses {
    next: AtomicU64,
    pending: Mutex<BTreeMap<u64, Cancellation>>,
}

impl Responses {
    fn start(&self) -> (u64, Cancellation) {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let cancellation = Cancellation::default();
        self.pending.lock().unwrap().insert(id, cancellation.clone());
        (id, cancellation)
    }

    fn finish(&self, id: u64) { self.pending.lock().unwrap().remove(&id); }

    pub fn active(&self) -> usize {
        self.pending.lock().unwrap().values().filter(|c| !c.is_cancelled()).count()
    }

    pub fn cancel(&self) {
        let pending: Vec<_> = self.pending.lock().unwrap().values().cloned().collect();
        for cancellation in pending { cancellation.cancel(); }
    }
}

/// Clones share one lease, so a voice-activated command can retain its processing slot.
#[derive(Clone)]
pub struct Job(Arc<Lease>);

struct Lease {
    app: AppHandle,
    id: u64,
    cancellation: Cancellation,
}

impl Job {
    pub fn start(app: &AppHandle) -> Self {
        let state = app.state::<crate::AppState>();
        let (id, cancellation) = state.responses.start();
        state.jobs.fetch_add(1, Ordering::SeqCst);
        crate::update_hud(app);
        Self(Arc::new(Lease { app: app.clone(), id, cancellation }))
    }

    pub fn cancellation(&self) -> Cancellation { self.0.cancellation.clone() }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(state) = self.app.try_state::<crate::AppState>() {
            state.responses.finish(self.id);
            state.jobs.fetch_sub(1, Ordering::SeqCst);
            crate::update_hud(&self.app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_stops_pending_work_and_is_not_lost_before_waiting() {
        let cancellation = Cancellation::default();
        let copy = cancellation.clone();
        let waiting = tokio::spawn(async move { copy.run(std::future::pending::<()>()).await });
        cancellation.cancel();
        assert_eq!(waiting.await.unwrap().unwrap_err(), CANCELLED);
        assert_eq!(cancellation.run(async { 1 }).await.unwrap_err(), CANCELLED);
        let mut pasted = false;
        assert_eq!(cancellation.run_if_active(|| { pasted = true; Ok(()) }).unwrap_err(), CANCELLED);
        assert!(!pasted);
    }

    #[tokio::test]
    async fn a_cancelled_server_request_cannot_deliver_a_late_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/transcription", listener.local_addr().unwrap());
        let (started, ready) = tokio::sync::oneshot::channel();
        let (finish, response_ready) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            started.send(()).unwrap();
            response_ready.await.unwrap();
            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nlate").await;
        });
        let cancellation = Cancellation::default();
        let copy = cancellation.clone();
        let delivered = Arc::new(AtomicBool::new(false));
        let pasted = delivered.clone();
        let client = tokio::spawn(async move {
            let http = reqwest::Client::builder().no_proxy().build().unwrap();
            let request = async { http.get(url).send().await.unwrap().text().await.unwrap() };
            let text = copy.run(request).await?;
            copy.run_if_active(|| { assert_eq!(text, "late"); pasted.store(true, Ordering::SeqCst); Ok(()) })
        });
        ready.await.unwrap();
        cancellation.cancel();
        assert_eq!(client.await.unwrap().unwrap_err(), CANCELLED);
        finish.send(()).unwrap();
        server.await.unwrap();
        assert!(!delivered.load(Ordering::SeqCst));
    }

    #[test]
    fn cancelling_responses_hides_them_without_cancelling_a_later_job() {
        let responses = Responses::default();
        let (first, first_cancel) = responses.start();
        let (second, second_cancel) = responses.start();
        assert_eq!(responses.active(), 2);
        responses.cancel();
        assert_eq!(responses.active(), 0);
        assert!(first_cancel.is_cancelled() && second_cancel.is_cancelled());
        let (next, next_cancel) = responses.start();
        responses.finish(first);
        responses.finish(second);
        assert_eq!(responses.active(), 1);
        assert!(!next_cancel.is_cancelled());
        responses.finish(next);
        assert_eq!(responses.active(), 0);
    }
}
