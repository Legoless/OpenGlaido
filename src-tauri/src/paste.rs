use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use crate::processing::{Cancellation, CANCELLED};
use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::sleep;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::Notify;

#[derive(Default)]
struct DeliveryOrder {
    next: u64,
    head: u64,
    finished: BTreeSet<u64>,
}

#[derive(Default)]
struct DeliveryQueue {
    order: Mutex<DeliveryOrder>,
    changed: Notify,
}

/// Reserve at recording stop, before starting parallel transcription. A dropped ticket releases its
/// place, including failed/cancelled jobs that finish before the earlier recording.
pub struct DeliveryTicket {
    queue: Arc<DeliveryQueue>,
    id: u64,
}

impl DeliveryQueue {
    fn reserve(self: &Arc<Self>) -> DeliveryTicket {
        let mut order = self.order.lock().unwrap_or_else(|e| e.into_inner());
        let id = order.next;
        order.next += 1;
        DeliveryTicket {
            queue: self.clone(),
            id,
        }
    }
}

pub fn reserve_delivery() -> DeliveryTicket {
    static QUEUE: LazyLock<Arc<DeliveryQueue>> = LazyLock::new(Arc::default);
    QUEUE.reserve()
}

impl DeliveryTicket {
    /// Keep the ticket alive until the paste completes. Waiting doesn't block transcription of
    /// later recordings; it only orders their delivery.
    pub async fn wait(&self) {
        loop {
            let changed = self.queue.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self
                .queue
                .order
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .head
                == self.id
            {
                return;
            }
            changed.await;
        }
    }
}

impl Drop for DeliveryTicket {
    fn drop(&mut self) {
        let mut order = self.queue.order.lock().unwrap_or_else(|e| e.into_inner());
        order.finished.insert(self.id);
        loop {
            let head = order.head;
            if !order.finished.remove(&head) {
                break;
            }
            order.head += 1;
        }
        drop(order);
        self.queue.changed.notify_waiters();
    }
}

/// Explicit command paste into the focused app.
pub fn paste_text(app: &AppHandle, text: &str, keep_in_clipboard: bool) -> Result<(), String> {
    paste(app, text, keep_in_clipboard, None, Cancellation::default())
}

/// Automatic dictation delivery requires the app captured at recording start to still be focused.
/// If it changed (or couldn't be identified), keep the transcript on the clipboard for manual paste.
pub fn paste_text_to(
    app: &AppHandle,
    text: &str,
    keep_in_clipboard: bool,
    target_app_id: Option<&str>,
) -> Result<(), String> {
    paste_text_to_cancellable(app, text, keep_in_clipboard, target_app_id, Cancellation::default())
}

pub fn paste_text_to_cancellable(
    app: &AppHandle,
    text: &str,
    keep_in_clipboard: bool,
    target_app_id: Option<&str>,
    cancellation: Cancellation,
) -> Result<(), String> {
    paste(
        app,
        text,
        keep_in_clipboard,
        Some(target_app_id.unwrap_or_default().to_owned()),
        cancellation,
    )
}

// ponytail: only text is restored; a previous image/file clipboard is lost. Snapshot all formats if users hit it.
fn paste(
    app: &AppHandle,
    text: &str,
    keep_in_clipboard: bool,
    target_app_id: Option<String>,
    cancellation: Cancellation,
) -> Result<(), String> {
    // Serialize the entire clipboard transaction, including the target app's time to read it.
    static TRANSACTION: Mutex<()> = Mutex::new(());
    let _transaction = TRANSACTION.lock().unwrap_or_else(|e| e.into_inner());
    cancellation.check()?;
    let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard error: {}", e))?;
    // Even "keep on clipboard" restores previous text when cancellation wins before insertion.
    let previous = clipboard.get_text().ok();
    cancellation.check()?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("Failed to set clipboard text: {}", e))?;
    let owned_version = verified_clipboard_version(text, clipboard_version, || clipboard.get_text().ok())
        .ok_or(CLIPBOARD_CHANGED)?;
    sleep(Duration::from_millis(50));

    // Destination and ownership are checked on the same thread immediately before posting Cmd+V.
    // On failure, do not restore: preserve any newer clipboard content. A focus-only
    // failure leaves the transcript available for manual paste.
    let result = send_paste_shortcut(app, target_app_id, owned_version, text.to_owned(), cancellation.clone());
    if result.is_err() && cancellation.is_cancelled() {
        restore_previous(&mut clipboard, previous, text, owned_version);
        return Err(CANCELLED.into());
    }
    result?;

    // ponytail: fixed 1 s for the target to read; slow remote desktops may need longer.
    sleep(Duration::from_millis(1000));
    if !keep_in_clipboard {
        restore_previous(&mut clipboard, previous, text, owned_version);
    }
    Ok(())
}

fn restore_previous(clipboard: &mut Clipboard, previous: Option<String>, text: &str, owned_version: u64) {
    if let Some(previous) = previous {
        // Also read back the text: a write between set_text and the initial version sample
        // must not let us adopt another app's clipboard content.
        if verified_clipboard_version(text, clipboard_version, || clipboard.get_text().ok()) == Some(owned_version) {
            if let Err(e) = clipboard.set_text(previous) {
                eprintln!("Failed to restore clipboard: {}", e);
            }
        }
    }
}

const CLIPBOARD_CHANGED: &str = "The clipboard changed before pasting; the transcript was not pasted";

/// Validate content with a stable native version both before and after readback. Unknown versions,
/// non-text data and concurrent writes fail closed. This isn't an atomic ownership token: a copy
/// of identical text before the first sample is indistinguishable from our own write.
fn verified_clipboard_version(
    expected: &str,
    mut version: impl FnMut() -> Option<u64>,
    read_text: impl FnOnce() -> Option<String>,
) -> Option<u64> {
    let before = version()?;
    if read_text().as_deref() != Some(expected) {
        return None;
    }
    (version() == Some(before)).then_some(before)
}

fn destination_matches(expected: Option<&str>, current: Option<&str>) -> bool {
    expected.is_none_or(|expected| !expected.is_empty() && current == Some(expected))
}

fn clipboard_version() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        Some(objc2_app_kit::NSPasteboard::generalPasteboard().changeCount() as u64)
    }
    #[cfg(target_os = "windows")]
    {
        let version = unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        (version != 0).then_some(version as u64)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

fn checked_paste(target_app_id: Option<&str>, owned_version: u64, text: &str, cancellation: &Cancellation) -> Result<(), String> {
    cancellation.check()?;
    let current = verified_clipboard_version(text, clipboard_version, || Clipboard::new().ok()?.get_text().ok());
    if current != Some(owned_version) {
        return Err(CLIPBOARD_CHANGED.into());
    }
    if !destination_matches(target_app_id, crate::frontmost::current_app_id().as_deref()) {
        return Err(
            "The focused app changed; the text is on the clipboard".into(),
        );
    }
    // Cancellation and the actual keyboard injection share one admission boundary. A cancel
    // that wins cannot leave a queued main-thread closure posting Cmd+V later.
    cancellation.run_if_active(press_paste)
}

/// ⌘V / Ctrl+V. On macOS enigo resolves "v" through the keyboard layout (Text Input Sources),
/// which macOS only allows on the main thread — so it runs there (and still works on Dvorak/AZERTY).
fn send_paste_shortcut(
    app: &AppHandle,
    target_app_id: Option<String>,
    owned_version: u64,
    text: String,
    cancellation: Cancellation,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::sync::atomic::{AtomicU8, Ordering::SeqCst};
        use std::sync::Arc;
        // 0 queued, 1 pressing, 2 given up: a paste that timed out must not fire later into
        // whatever app is frontmost by then (the text stays on the clipboard instead).
        let phase = Arc::new(AtomicU8::new(0));
        let (tx, rx) = std::sync::mpsc::channel();
        let queued = phase.clone();
        app.run_on_main_thread(move || {
            if queued.compare_exchange(0, 1, SeqCst, SeqCst).is_ok() {
                let _ = tx.send(checked_paste(target_app_id.as_deref(), owned_version, &text, &cancellation));
            }
        })
        .map_err(|e| format!("Couldn't paste: {e}"))?;
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(result) => result,
            Err(_) if phase.compare_exchange(0, 2, SeqCst, SeqCst).is_ok() => {
                Err("Paste timed out; the text is on the clipboard".into())
            }
            // Already pressing: wait for it.
            Err(_) => rx.recv().unwrap_or_else(|_| Err("Paste failed".into())),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        checked_paste(target_app_id.as_deref(), owned_version, &text, &cancellation)
    }
}

fn press_paste() -> Result<(), String> {
    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("Enigo error: {:?}", e))?;
    let modifier = if cfg!(target_os = "macos") {
        Key::Meta
    } else {
        Key::Control
    };
    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| format!("{:?}", e))?;
    let pressed = enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| format!("{:?}", e));
    // Always release the modifier, even if the V press failed.
    enigo
        .key(modifier, Direction::Release)
        .map_err(|e| format!("{:?}", e))?;
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_paste_stops_before_any_clipboard_or_keyboard_access() {
        let cancellation = Cancellation::default();
        cancellation.cancel();
        assert_eq!(checked_paste(None, 0, "must not be pasted", &cancellation).unwrap_err(), CANCELLED);
    }

    #[tokio::test]
    async fn delivery_is_ordered_and_cancelled_slots_never_block_later_jobs() {
        let queue = Arc::new(DeliveryQueue::default());
        let first = queue.reserve();
        let cancelled = queue.reserve();
        let third = queue.reserve();
        let fourth = queue.reserve();
        first.wait().await;
        assert!(tokio::time::timeout(Duration::from_millis(5), third.wait())
            .await
            .is_err());
        drop(cancelled); // Finishing slot 2 early must not release slot 3 ahead of slot 1.
        assert!(tokio::time::timeout(Duration::from_millis(5), third.wait())
            .await
            .is_err());
        drop(first);
        tokio::time::timeout(Duration::from_secs(1), third.wait())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(5), fourth.wait())
                .await
                .is_err()
        );
        drop(third);
        tokio::time::timeout(Duration::from_secs(1), fourth.wait())
            .await
            .unwrap();
        drop(fourth);
        assert!(queue.order.lock().unwrap().finished.is_empty());
        let next = queue.reserve();
        tokio::time::timeout(Duration::from_secs(1), next.wait())
            .await
            .unwrap();
    }

    #[test]
    fn matching_clipboard_text_needs_a_stable_native_version() {
        let text = || Some("dictation".to_owned());
        let owned = verified_clipboard_version("dictation", || Some(10), text);
        assert_eq!(owned, Some(10));
        // A fresh copy of identical text after our initial sample loses ownership.
        assert_ne!(verified_clipboard_version("dictation", || Some(11), text), owned);
        assert_eq!(verified_clipboard_version("dictation", || None, text), None);
        let version = std::cell::Cell::new(10);
        assert_eq!(verified_clipboard_version("dictation", || Some(version.get()), || {
            version.set(11); // Another app copies while readback is in progress.
            text()
        }), None);
    }

    #[test]
    fn replacement_between_write_and_initial_version_sample_is_not_adopted() {
        // These fake readbacks model another app replacing our write before changeCount is read.
        for replacement in [Some("another copy".to_owned()), None] {
            let observed = verified_clipboard_version("dictation", || Some(11), || replacement);
            assert_eq!(observed, None, "neither unrelated text nor an image may establish ownership");
        }
    }

    #[test]
    fn automatic_delivery_requires_the_same_target() {
        assert!(destination_matches(Some("editor"), Some("editor")));
        assert!(!destination_matches(Some("editor"), Some("mail")));
        assert!(!destination_matches(Some("editor"), None));
        assert!(!destination_matches(Some(""), Some("")));
        assert!(destination_matches(None, Some("editor"))); // Explicit command paste.
    }
}
