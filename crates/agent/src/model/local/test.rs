use super::*;

#[test]
fn nothing_configured_is_no_local_server() {
    assert_eq!(LocalLlm::from_values(None, None, None).unwrap(), None);
    assert_eq!(
        LocalLlm::from_values(Some("  "), Some(""), Some(" ")).unwrap(),
        None
    );
}

#[test]
fn a_base_url_alone_serves_only_local_ids() {
    let local = LocalLlm::from_values(Some("http://localhost:11434/v1"), None, None)
        .unwrap()
        .unwrap();

    assert_eq!(local.base_url(), "http://localhost:11434/v1");
    assert_eq!(local.api_key(), KEYLESS_BEARER);
    assert_eq!(local.model(), None);
}

#[test]
fn values_are_trimmed_and_a_model_replaces_the_others() {
    let local = LocalLlm::from_values(
        Some(" http://localhost:8080/v1 "),
        Some(" secret "),
        Some(" qwen3:30b "),
    )
    .unwrap()
    .unwrap();

    assert_eq!(local.base_url(), "http://localhost:8080/v1");
    assert_eq!(local.api_key(), "secret");
    assert_eq!(local.model(), Some("qwen3:30b"));
}

#[test]
fn a_model_without_a_server_is_a_configuration_error() {
    let error = LocalLlm::from_values(None, None, Some("qwen3:30b")).unwrap_err();

    assert!(error.to_string().contains("LOCAL_LLM_BASE_URL"));
}

#[test]
fn debug_never_prints_the_key() {
    let local = LocalLlm::from_values(Some("http://x/v1"), Some("hunter2"), None)
        .unwrap()
        .unwrap();

    assert!(!format!("{local:?}").contains("hunter2"));
}
