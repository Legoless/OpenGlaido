use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::thread::sleep;
use std::time::Duration;

pub fn paste_text(text: &str) -> Result<(), String> {
    // 1. Copy text to system clipboard
    let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard error: {}", e))?;
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

    Ok(())
}
