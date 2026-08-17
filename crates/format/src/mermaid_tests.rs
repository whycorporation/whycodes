use super::*;

#[test]
fn recognises_mermaid_fence_tags() {
    assert!(is_mermaid_language(Some("mermaid")));
    assert!(is_mermaid_language(Some("Mermaid")));
    assert!(is_mermaid_language(Some("mmd")));
    assert!(!is_mermaid_language(Some("rust")));
    assert!(!is_mermaid_language(None));
}

#[test]
fn renders_a_simple_flowchart() {
    let lines = render_mermaid("graph LR; A[Build] --> B[Deploy]", None).unwrap();
    let joined = lines.join("\n");
    assert!(joined.contains("Build"), "{joined}");
    assert!(joined.contains("Deploy"), "{joined}");
    #[cfg(feature = "mermaid")]
    assert!(!joined.contains("graph LR"), "{joined}");
    #[cfg(not(feature = "mermaid"))]
    assert!(joined.contains("graph LR"), "{joined}");
}

#[test]
fn renders_sequence_diagrams() {
    let src = "sequenceDiagram\n    Alice->>Bob: Hello\n    Bob-->>Alice: Hi";
    let lines = render_mermaid(src, None).unwrap();
    let joined = lines.join("\n");
    assert!(joined.contains("Alice"), "{joined}");
    assert!(joined.contains("Bob"), "{joined}");
}

#[test]
fn empty_source_is_an_error() {
    assert!(render_mermaid("   \n  ", None).is_err());
}

#[test]
#[cfg(feature = "mermaid")]
fn unsupported_or_garbage_falls_to_err() {
    // Completely blank after comments, or unknown type — either way Err.
    let result = render_mermaid("this is not mermaid at all", None);
    assert!(result.is_err(), "{result:?}");
}

#[test]
#[cfg(not(feature = "mermaid"))]
fn source_fallback_keeps_garbage_readable() {
    let lines = render_mermaid("this is not mermaid at all", None).unwrap();
    assert_eq!(lines.join("\n"), "this is not mermaid at all");
}

#[test]
fn closed_cache_returns_identical_lines() {
    let src = "graph TD; A --> B";
    let a = render_mermaid(src, Some(80)).unwrap();
    let b = render_mermaid(src, Some(80)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn cache_evicts_when_full() {
    for i in 0..40 {
        let _ = render_mermaid(&format!("graph TD; A{i} --> B{i}"), None);
    }
    let again = render_mermaid("graph TD; A0 --> B0", None);
    assert!(again.is_ok());
}
