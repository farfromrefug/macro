//! Instance tokens as a source of connections: a vendor API token the
//! operator put in the process's environment, which counts as every user's
//! connection to that app.
//!
//! It exists for installations with no hosted broker (Pipedream), where the
//! importers that read Linear or Notion over their REST APIs would otherwise
//! have no connection. It provides API access, and MCP for the apps whose
//! hosted MCP server takes the token as a bearer (Linear), and no per-user
//! state, so nothing is stored and nothing to revoke. Connecting, renaming,
//! turning off and removing are refused; the operator changes the token by
//! changing the environment.

use crate::domain::models::{
    ApiRequest, ApiResponse, ConnectStep, ConnectorId, ConnectorKey, Surfaces, Upstream,
};
use crate::domain::ports::{
    ApiConnection, GrantState, McpConnection, Source, SourceConnection, SourceKind, SourceRefusal,
};
use crate::outbound::guarded_http::McpHttp;
use crate::outbound::native::models::{McpServer, dial_with_bearer};
use http::HeaderValue;
use http::header::AUTHORIZATION;
use macro_user_id::user_id::MacroUserIdStr;
use rootcause::Report;
use rootcause::compat::IntoRootcause as _;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

#[cfg(test)]
mod test;

/// How long one call to a vendor may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The prefix of a Linear personal API key, which Linear takes as the whole
/// `Authorization` value; its OAuth tokens take `Bearer`.
const LINEAR_PERSONAL_KEY_PREFIX: &str = "lin_api_";

/// A vendor API token. Its `Debug` never shows it.
#[derive(Clone)]
struct ApiToken(String);

impl fmt::Debug for ApiToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApiToken(..)")
    }
}

/// An app an instance token can be given for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenApp {
    Linear,
    Notion,
}

impl TokenApp {
    /// The app's catalog slug.
    fn slug(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Notion => "notion",
        }
    }

    /// The name connections to the app show.
    fn name(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::Notion => "Notion",
        }
    }

    /// The only origin the token is ever sent to.
    fn origin(self) -> &'static str {
        match self {
            Self::Linear => "https://api.linear.app",
            Self::Notion => "https://api.notion.com",
        }
    }

    /// The app's hosted MCP server, when it takes the token as a bearer.
    /// Notion's takes OAuth sign-ins only, not an integration token.
    fn mcp_url(self) -> Option<&'static str> {
        match self {
            Self::Linear => Some("https://mcp.linear.app/mcp"),
            Self::Notion => None,
        }
    }

    /// What a connection to the app can be used as.
    fn surfaces(self) -> Surfaces {
        Surfaces::new(self.mcp_url().is_some(), true)
    }
}

/// A token the operator gave an app for the whole instance.
#[derive(Clone, Debug)]
pub struct TokenAppConfig {
    app: TokenApp,
    token: ApiToken,
    /// Replaces the app's origin, for tests that stand in for the vendor.
    origin: Option<Url>,
}

impl TokenAppConfig {
    /// A Linear personal API key (`lin_api_...`) or OAuth access token.
    pub fn linear(token: &str) -> Self {
        Self::new(TokenApp::Linear, token)
    }

    /// A Notion internal integration token.
    pub fn notion(token: &str) -> Self {
        Self::new(TokenApp::Notion, token)
    }

    fn new(app: TokenApp, token: &str) -> Self {
        Self {
            app,
            token: ApiToken(token.trim().to_owned()),
            origin: None,
        }
    }

    /// Whether there is a token to use.
    pub fn is_set(&self) -> bool {
        !self.token.0.is_empty()
    }
}

/// The `Authorization` value `token` takes at `app`.
fn authorization(app: TokenApp, token: &ApiToken) -> Result<HeaderValue, Report> {
    let value = match app {
        TokenApp::Linear if token.0.starts_with(LINEAR_PERSONAL_KEY_PREFIX) => token.0.clone(),
        TokenApp::Linear | TokenApp::Notion => format!("Bearer {}", token.0),
    };
    let mut value = HeaderValue::from_str(&value).map_err(Report::new)?;
    value.set_sensitive(true);
    Ok(value)
}

/// One app's token, and the calls it may make.
struct TokenConnection {
    kind: TokenApp,
    app: ConnectorId,
    token: ApiToken,
    origin: Url,
    http: reqwest::Client,
    /// The app's MCP server, when it takes the token.
    mcp_url: Option<Url>,
    mcp_http: McpHttp,
}

impl TokenConnection {
    /// The MCP server and token, or why this app has none.
    fn mcp_target(&self) -> Result<(&Url, &str), Report> {
        let url = self.mcp_url.as_ref().ok_or_else(|| {
            rootcause::report!(
                "{} has no MCP server for an instance token",
                self.kind.name()
            )
        })?;
        Ok((url, self.token.0.as_str()))
    }

    /// `url` as a URL on this app's origin and nowhere else: the same scheme,
    /// host and port, and no user info to confuse the host with.
    fn pinned(&self, url: &str) -> Result<Url, Report> {
        let url = Url::parse(url).map_err(Report::new)?;
        let on_origin = url.scheme() == self.origin.scheme()
            && url.host_str() == self.origin.host_str()
            && url.port_or_known_default() == self.origin.port_or_known_default()
            && url.username().is_empty()
            && url.password().is_none();
        if !on_origin {
            return Err(rootcause::report!(
                "the {} token is only sent to {}",
                self.kind.name(),
                self.origin
            ));
        }

        Ok(url)
    }
}

#[async_trait::async_trait]
impl McpConnection for TokenConnection {
    async fn dial(&self) -> Result<McpServer, Report> {
        let (url, token) = self.mcp_target()?;
        dial_with_bearer(&self.mcp_http, url, token)
            .await
            .into_rootcause()
    }

    async fn upstream(&self) -> Result<Upstream, Report> {
        let (url, token) = self.mcp_target()?;
        Ok(Upstream::new(
            url.clone(),
            Some(token.to_owned()),
            http::HeaderMap::new(),
        ))
    }
}

#[async_trait::async_trait]
impl ApiConnection for TokenConnection {
    /// Call the app's API with the token. A non-2xx answer, a redirect
    /// included (never followed, so the token cannot leave the origin), is a
    /// response. No trace context is sent: the vendor is not a hop of ours.
    /// Never log the body: some apps return secrets.
    #[tracing::instrument(skip_all, err, fields(app = %self.app, method = ?request.method))]
    async fn call(&self, request: ApiRequest) -> Result<ApiResponse, Report> {
        let ApiRequest {
            method,
            url,
            headers,
            body,
        } = request;
        if headers.contains_key(AUTHORIZATION) {
            return Err(rootcause::report!(
                "the connection adds Authorization; callers may not set it"
            ));
        }

        let mut request = self
            .http
            .request(method, self.pinned(&url)?)
            .headers(headers)
            .header(AUTHORIZATION, authorization(self.kind, &self.token)?);
        if let Some(body) = &body {
            request = request.json(body);
        }

        let response = request.send().await.map_err(Report::new)?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().await.map_err(Report::new)?;
        Ok(ApiResponse {
            status,
            headers,
            body,
        })
    }
}

/// The instance tokens as a [`Source`].
pub(crate) struct TokenSource {
    connections: Vec<Arc<TokenConnection>>,
}

impl TokenSource {
    /// A source over the tokens in `configs`, the last one winning for an app
    /// given twice. Blank tokens are left out.
    pub(crate) fn new(configs: Vec<TokenAppConfig>, mcp_http: McpHttp) -> Result<Self, Report> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(Report::new)?;

        let mut connections: Vec<Arc<TokenConnection>> = Vec::new();
        for config in configs.into_iter().filter(TokenAppConfig::is_set) {
            let origin = match config.origin {
                Some(origin) => origin,
                None => Url::parse(config.app.origin()).map_err(Report::new)?,
            };
            let connection = Arc::new(TokenConnection {
                kind: config.app,
                app: ConnectorId::parse(config.app.slug()).map_err(Report::new)?,
                token: config.token,
                origin,
                http: http.clone(),
                mcp_url: config
                    .app
                    .mcp_url()
                    .map(Url::parse)
                    .transpose()
                    .map_err(Report::new)?,
                mcp_http: mcp_http.clone(),
            });
            connections.retain(|held| held.kind != config.app);
            connections.push(connection);
        }

        Ok(Self { connections })
    }

    /// The apps this source holds a token for.
    pub(crate) fn apps(&self) -> HashMap<ConnectorId, Surfaces> {
        self.connections
            .iter()
            .map(|connection| (connection.app.clone(), connection.kind.surfaces()))
            .collect()
    }

    fn serving(&self, app: &ConnectorId) -> Option<&Arc<TokenConnection>> {
        self.connections
            .iter()
            .find(|connection| connection.app == *app)
    }
}

/// The refusal of an operation this source never takes: the operator manages
/// its tokens, not the user.
fn refused(key: ConnectorKey) -> Report {
    Report::new(SourceRefusal::Unsupported(key)).into_dynamic()
}

#[async_trait::async_trait]
impl Source for TokenSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Token
    }

    /// One connection to each app with a token, for every user.
    async fn connections(
        &self,
        _user: &MacroUserIdStr<'static>,
    ) -> Result<Vec<SourceConnection>, Report> {
        Ok(self
            .connections
            .iter()
            .map(|connection| SourceConnection {
                source: SourceKind::Token,
                key: ConnectorKey::App(connection.app.clone()),
                source_id: connection.app.to_string(),
                name: connection.kind.name().to_owned(),
                enabled: true,
                grant: GrantState::Working,
                mcp: connection
                    .mcp_url
                    .is_some()
                    .then(|| connection.clone() as Arc<dyn McpConnection>),
                api: Some(connection.clone()),
            })
            .collect())
    }

    /// The REST API of each app with a token, and its MCP server where the
    /// token opens one.
    async fn supports(&self, app: &ConnectorId) -> Result<Option<Surfaces>, Report> {
        Ok(self
            .serving(app)
            .map(|connection| connection.kind.surfaces()))
    }

    /// Nobody: the token belongs to the instance, so there are no per-user
    /// rows to list, and the importers that use it start from a user.
    async fn holders(&self, _app: &ConnectorId) -> Result<Vec<MacroUserIdStr<'static>>, Report> {
        Ok(Vec::new())
    }

    async fn start_connect(
        &self,
        _user: &MacroUserIdStr<'static>,
        key: &ConnectorKey,
    ) -> Result<ConnectStep, Report> {
        Err(refused(key.clone()))
    }

    async fn complete_connect(
        &self,
        _user: &MacroUserIdStr<'static>,
        app: &ConnectorId,
    ) -> Result<SourceConnection, Report> {
        Err(refused(ConnectorKey::App(app.clone())))
    }

    async fn set_enabled(
        &self,
        _user: &MacroUserIdStr<'static>,
        key: &ConnectorKey,
        _enabled: bool,
    ) -> Result<(), Report> {
        Err(refused(key.clone()))
    }

    async fn rename(
        &self,
        _user: &MacroUserIdStr<'static>,
        key: &ConnectorKey,
        _name: &str,
    ) -> Result<(), Report> {
        Err(refused(key.clone()))
    }

    async fn remove(
        &self,
        _user: &MacroUserIdStr<'static>,
        key: &ConnectorKey,
    ) -> Result<(), Report> {
        Err(refused(key.clone()))
    }
}
