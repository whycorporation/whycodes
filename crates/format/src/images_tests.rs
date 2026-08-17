use super::*;

#[test]
fn test_render_with_alt() {
    let result = render_image_ref("screenshot of code", "https://example.com/img.png");
    assert!(result.contains("screenshot of code"));
    assert!(result.contains("https://example.com/img.png"));
    assert!(result.contains("\x1b[2m"));
}

#[test]
fn test_render_empty_alt() {
    let result = render_image_ref("", "https://example.com/img.png");
    assert!(result.contains("image"));
}
