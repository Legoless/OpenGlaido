use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread::{self, sleep};
use std::time::Duration;
use tauri::AppHandle;

/// Pastes `text` into the focused app via the clipboard. Unless `keep_in_clipboard`, the previous
/// clipboard text is restored ~1 s later on a background thread.
// ponytail: only text is restored; a previous image/file clipboard is lost. Snapshot all formats if users hit it.
pub fn paste_text(app: &AppHandle, text: &str, keep_in_clipboard: bool) -> Result<(), String> {
    // 1. Copy text to system clipboard
    let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard error: {}", e))?;
    let previous = if keep_in_clipboard { None } else { clipboard.get_text().ok() };
    clipboard
        .set_text(text)
        .map_err(|e| format!("Failed to set clipboard text: {}", e))?;

    // Brief sleep to ensure clipboard buffer settles
    sleep(Duration::from_millis(50));

    // 2. Simulate native Cmd+V (macOS) or Ctrl+V (Windows/Linux)
    send_paste_shortcut(app)?;

    // 3. Give the target app time to read the clipboard, then put the user's text back.
    // ponytail: fixed 1 s; slow apps (remote desktops) may need longer. Skipped if the user copied meanwhile.
    if let Some(previous) = previous {
        let pasted = text.to_string();
        thread::spawn(move || {
            sleep(Duration::from_millis(1000));
            let restore = Clipboard::new().and_then(|mut c| match c.get_text() {
                Ok(current) if current != pasted => Ok(()),
                _ => c.set_text(previous),
            });
            if let Err(e) = restore {
                eprintln!("Failed to restore clipboard: {}", e);
            }
        });
    }

    Ok(())
}


/// ⌘V / Ctrl+V. On macOS enigo resolves "v" through the keyboard layout (Text Input Sources),
/// which macOS only allows on the main thread — so it runs there (and still works on Dvorak/AZERTY).
fn send_paste_shortcut(app: &AppHandle) -> Result<(), String> {
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
                let _ = tx.send(press_paste());
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
        press_paste()
    }
}

fn press_paste() -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Enigo error: {:?}", e))?;
    let modifier = if cfg!(target_os = "macos") { Key::Meta } else { Key::Control };
    enigo.key(modifier, Direction::Press).map_err(|e| format!("{:?}", e))?;
    let pressed = enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| format!("{:?}", e));
    // Always release the modifier, even if the V press failed.
    enigo.key(modifier, Direction::Release).map_err(|e| format!("{:?}", e))?;
    pressed
}
