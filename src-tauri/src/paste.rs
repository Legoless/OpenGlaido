use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread::{self, sleep};
use std::time::Duration;

/// Pastes `text` into the focused app via the clipboard. Unless `keep_in_clipboard`, the previous
/// clipboard text is restored ~1 s later on a background thread.
// ponytail: only text is restored; a previous image/file clipboard is lost. Snapshot all formats if users hit it.
pub fn paste_text(text: &str, keep_in_clipboard: bool) -> Result<(), String> {
    // 1. Copy text to system clipboard
    let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard error: {}", e))?;
    let previous = if keep_in_clipboard { None } else { clipboard.get_text().ok() };
    clipboard
        .set_text(text)
        .map_err(|e| format!("Failed to set clipboard text: {}", e))?;

    // Brief sleep to ensure clipboard buffer settles
    sleep(Duration::from_millis(50));

    // 2. Simulate native Cmd+V (macOS) or Ctrl+V (Windows/Linux)
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Enigo error: {:?}", e))?;

    #[cfg(target_os = "macos")]
    {
        enigo.key(Key::Meta, Direction::Press).map_err(|e| format!("{:?}", e))?;
        enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| format!("{:?}", e))?;
        enigo.key(Key::Meta, Direction::Release).map_err(|e| format!("{:?}", e))?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        enigo.key(Key::Control, Direction::Press).map_err(|e| format!("{:?}", e))?;
        enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| format!("{:?}", e))?;
        enigo.key(Key::Control, Direction::Release).map_err(|e| format!("{:?}", e))?;
    }

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
