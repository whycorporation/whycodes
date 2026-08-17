/// Render an image reference as a dimmed terminal string.
///
/// Returns a string like `[Image: alt text]` with ANSI dim/bold styling
/// so it stands out from regular text but doesn't distract.
pub fn render_image_ref(alt: &str, url: &str) -> String {
    let alt = if alt.is_empty() { "image" } else { alt };
    format!("\x1b[2m[Image: \x1b[1m{alt}\x1b[0m\x1b[2m]\x1b[0m (url: {url})")
}

#[cfg(test)]
#[path = "images_tests.rs"]
mod tests;
