//! Putting text on the system clipboard.

use std::sync::Mutex;

/// The one clipboard handle, kept for as long as the program runs.
///
/// On Linux the program that copied is the one that hands the text over when
/// something is pasted, so a handle dropped straight after copying can take
/// the text with it. Keeping it avoids that.
static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

/// Put `text` on the clipboard, or say why it could not be.
pub fn copy_text(text: &str) -> Result<(), String> {
    let mut held = CLIPBOARD
        .lock()
        .map_err(|_| "the clipboard is in use".to_string())?;
    if held.is_none() {
        *held = Some(arboard::Clipboard::new().map_err(|e| format!("no clipboard: {e}"))?);
    }
    held.as_mut()
        .expect("just set")
        .set_text(text.to_string())
        .map_err(|e| format!("could not copy: {e}"))
}
