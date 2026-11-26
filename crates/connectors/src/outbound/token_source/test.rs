use super::*;
use crate::outbound::guarded_http::AddressPolicy;
use http::{HeaderMap, Method};
use macro_user_id::cowlike::CowLike;
use std::collections::HashSet;

const LINEAR_KEY: &str = "lin_api_abc123";
const NOTION_TOKEN: &str = "ntn_secret";

fn user(email: &str) -> MacroUserIdStr<'static> {
    MacroUserIdStr::parse_from_str(&format!("macro|{email}"))
        .unwrap()
        .into_owned()
}

fn app(slug: &str) -> ConnectorId {
    ConnectorId::parse(slug).unwrap()
}

fn mcp_http() -> McpHttp {
    McpHttp::new(AddressPolicy::PublicOnly).unwrap()
}

fn source() -> TokenSource {
    TokenSource::new(
        vec![
            TokenAppConfig::linear(LINEAR_KEY),
            TokenAppConfig::notion(NOTION_TOKEN),
        ],
        mcp_http(),
    )
    .unwrap()
}

fn connection(source: &TokenSource, slug: &str) -> Arc<TokenConnection> {
    source.serving(&app(slug)).unwrap().clone()
}

fn request(url: &str) -> ApiRequest {
    ApiRequest {
        method: Method::POST,
        url: url.to_owned(),
        headers: HeaderMap::new(),
        body: None,
    }
}

#[test]
fn only_the_apps_origin_is_pinned() {
    let linear = connection(&source(), "linear");

    assert!(linear.pinned("https://api.linear.app/graphql").is_ok());
    assert!(linear.pinned("https://API.Linear.app:443/graphql").is_ok());
    for refused in [
        "http://api.linear.app/graphql",
        "https://api.notion.com/v1/search",
        "https://api.linear.app.evil.com/graphql",
        "https://evil.com/api.linear.app",
        "https://api.linear.app@evil.com/graphql",
        "https://evil.com@api.linear.app/graphql",
        "https://user:pass@api.linear.app/graphql",
        "https://api.linear.app:8443/graphql",
        "not a url",
    ] {
        assert!(linear.pinned(refused).is_err(), "{refused} was accepted");
    }
}

#[test]
fn each_app_is_pinned_to_its_own_vendor() {
    let notion = connection(&source(), "notion");

    assert!(notion.pinned("https://api.notion.com/v1/search").is_ok());
    assert!(notion.pinned("https://api.linear.app/graphql").is_err());
}

#[test]
fn a_linear_personal_key_is_the_whole_authorization_value() {
    let value = authorization(TokenApp::Linear, &ApiToken(LINEAR_KEY.to_owned())).unwrap();

    assert_eq!(value.to_str().unwrap(), LINEAR_KEY);
    assert!(value.is_sensitive());
}

#[test]
fn other_linear_tokens_and_notion_tokens_are_bearer() {
    let linear = authorization(TokenApp::Linear, &ApiToken("oauth-token".to_owned())).unwrap();
    let notion = authorization(TokenApp::Notion, &ApiToken(LINEAR_KEY.to_owned())).unwrap();

    assert_eq!(linear.to_str().unwrap(), "Bearer oauth-token");
    assert_eq!(notion.to_str().unwrap(), format!("Bearer {LINEAR_KEY}"));
}

#[test]
fn a_token_that_cannot_be_a_header_value_is_refused() {
    assert!(authorization(TokenApp::Notion, &ApiToken("bad\ntoken".to_owned())).is_err());
}

#[test]
fn tokens_are_trimmed_and_blank_ones_leave_the_app_out() {
    let source = TokenSource::new(
        vec![
            TokenAppConfig::linear("  \n"),
            TokenAppConfig::notion(&format!("{NOTION_TOKEN}\n")),
        ],
        mcp_http(),
    )
    .unwrap();

    assert_eq!(
        source.apps().into_keys().collect::<HashSet<_>>(),
        HashSet::from([app("notion")])
    );
    assert_eq!(connection(&source, "notion").token.0, NOTION_TOKEN);
}

#[test]
fn an_app_given_twice_keeps_the_last_token() {
    let source = TokenSource::new(
        vec![
            TokenAppConfig::notion("first"),
            TokenAppConfig::notion("last"),
        ],
        mcp_http(),
    )
    .unwrap();

    assert_eq!(source.connections.len(), 1);
    assert_eq!(connection(&source, "notion").token.0, "last");
}

#[test]
fn a_token_never_shows_in_debug_output() {
    let config = TokenAppConfig::linear(LINEAR_KEY);

    assert!(!format!("{config:?}").contains(LINEAR_KEY));
}

#[tokio::test]
async fn every_user_holds_a_working_connection_to_each_app() {
    let source = source();

    for email in ["a@example.com", "b@example.com"] {
        let held = source.connections(&user(email)).await.unwrap();

        let keys: Vec<_> = held.iter().map(|held| held.key.clone()).collect();
        assert_eq!(
            keys,
            vec![
                ConnectorKey::App(app("linear")),
                ConnectorKey::App(app("notion"))
            ]
        );
        for held in held {
            assert_eq!(held.source, SourceKind::Token);
            assert!(held.enabled);
            assert_eq!(held.grant, GrantState::Working);
            assert_eq!(
                held.mcp.is_some(),
                held.key == ConnectorKey::App(app("linear"))
            );
            assert!(held.api.is_some());
        }
    }
}

#[tokio::test]
async fn linear_serves_its_hosted_mcp_server_with_the_token_as_bearer() {
    let source = source();
    let held = source.connections(&user("a@example.com")).await.unwrap();
    let linear = held
        .into_iter()
        .find(|held| held.key == ConnectorKey::App(app("linear")))
        .unwrap();

    let upstream = linear.mcp.unwrap().upstream().await.unwrap();

    assert_eq!(upstream.url().as_str(), "https://mcp.linear.app/mcp");
    assert_eq!(upstream.bearer(), Some(LINEAR_KEY));
}

#[tokio::test]
async fn notion_has_no_mcp_for_an_integration_token() {
    let source = source();

    assert_eq!(
        source.supports(&app("notion")).await.unwrap(),
        Some(Surfaces::new(false, true))
    );
    assert_eq!(
        source.supports(&app("linear")).await.unwrap(),
        Some(Surfaces::new(true, true))
    );
    assert!(connection(&source, "notion").mcp_target().is_err());
}

#[tokio::test]
async fn it_supports_only_the_apps_with_a_token() {
    let source = TokenSource::new(vec![TokenAppConfig::notion(NOTION_TOKEN)], mcp_http()).unwrap();

    assert_eq!(
        source.supports(&app("notion")).await.unwrap(),
        Some(Surfaces::new(false, true))
    );
    assert_eq!(source.supports(&app("linear")).await.unwrap(), None);
    assert!(source.holders(&app("notion")).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_user_cannot_change_an_instance_token() {
    let source = source();
    let user = user("a@example.com");
    let key = ConnectorKey::App(app("notion"));
    let refusals = [
        source.start_connect(&user, &key).await.err(),
        source.complete_connect(&user, &app("notion")).await.err(),
        source.set_enabled(&user, &key, false).await.err(),
        source.rename(&user, &key, "Mine").await.err(),
        source.remove(&user, &key).await.err(),
    ];

    for refusal in refusals {
        let refusal = refusal.expect("the operation is refused");
        assert_eq!(
            refusal.downcast_current_context::<SourceRefusal>(),
            Some(&SourceRefusal::Unsupported(key.clone()))
        );
    }
}

#[tokio::test]
async fn a_call_off_the_vendors_origin_is_refused_before_it_is_sent() {
    let linear = connection(&source(), "linear");

    assert!(
        linear
            .call(request("https://api.notion.com/v1/search"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_caller_cannot_set_authorization() {
    let linear = connection(&source(), "linear");
    let mut request = request("https://api.linear.app/graphql");
    request
        .headers
        .insert(AUTHORIZATION, HeaderValue::from_static("Bearer other"));

    assert!(linear.call(request).await.is_err());
}
