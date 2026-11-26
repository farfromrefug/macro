//! App identity: the slug aliases (rule 4), the overlay lookup that decides
//! when a native server is an app (rule 5), and which apps Macro Connect
//! serves.

use crate::domain::models::{ConnectorId, ServerUrl, Surfaces};
use std::collections::{BTreeSet, HashMap, HashSet};

#[cfg(test)]
mod test;

/// GitHub's slug, the app the Macro account link connects.
pub(crate) const GITHUB_SLUG: &str = "github";

/// Pipedream slugs that name the same app as another slug, with the slug
/// they fold to.
const ALIASES: &[(&str, &str)] = &[("slack_v2", "slack")];

/// The app a slug names: itself, or the app it is an alias of.
pub(crate) fn canonical_slug(slug: &str) -> &str {
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == slug)
        .map_or(slug, |(_, canonical)| canonical)
}

/// Every slug that names `app`: its own, then each alias that folds to it.
pub(crate) fn slugs_of(app: &ConnectorId) -> Vec<&str> {
    let aliases = ALIASES
        .iter()
        .filter(|(_, canonical)| *canonical == app.as_str())
        .map(|(alias, _)| *alias);

    std::iter::once(app.as_str()).chain(aliases).collect()
}

/// The routing table of apps that do not use the default source (Pipedream):
/// catalog apps served by the vendor's own MCP server (`mcp_overlays`), and
/// the apps the operator gave an instance token.
///
/// Addresses are not unique: several slugs can name one server, and a user's
/// one row at that server stands for every app there.
#[derive(Clone, Debug, Default)]
pub(crate) struct Overlays {
    servers: HashMap<ConnectorId, ServerUrl>,
    apps: HashMap<ServerUrl, BTreeSet<ConnectorId>>,
    token_apps: HashSet<ConnectorId>,
}

impl Overlays {
    /// The overlay table.
    pub(crate) fn new(entries: impl IntoIterator<Item = (ConnectorId, ServerUrl)>) -> Self {
        let mut servers = HashMap::new();
        let mut apps: HashMap<ServerUrl, BTreeSet<ConnectorId>> = HashMap::new();
        for (app, url) in entries {
            apps.entry(url.clone()).or_default().insert(app.clone());
            servers.insert(app, url);
        }

        Self {
            servers,
            apps,
            token_apps: HashSet::new(),
        }
    }

    /// These routes, plus `token_apps`: the apps the instance token source
    /// serves.
    pub(crate) fn with_token_apps(mut self, token_apps: &HashSet<ConnectorId>) -> Self {
        self.token_apps.clone_from(token_apps);
        self
    }

    /// Whether the instance token source serves `app`.
    pub(crate) fn is_token_app(&self, app: &ConnectorId) -> bool {
        self.token_apps.contains(app)
    }

    /// Whether `app` is overlaid.
    pub(crate) fn is_overlaid(&self, app: &ConnectorId) -> bool {
        self.servers.contains_key(app)
    }

    /// The server `app` is overlaid at, if it is overlaid.
    pub(crate) fn server_of(&self, app: &ConnectorId) -> Option<&ServerUrl> {
        self.servers.get(app)
    }

    /// Every overlaid app whose server is exactly `url`, in slug order;
    /// empty for a server added by URL, whatever it points at.
    pub(crate) fn apps_at(&self, url: &ServerUrl) -> impl Iterator<Item = &ConnectorId> {
        self.apps.get(url).into_iter().flatten()
    }
}

/// The apps Macro Connect serves, with the surfaces its catalog lists for
/// each. An app missing here is one Connect does not support.
#[derive(Clone, Debug, Default)]
pub(crate) struct ConnectSupport {
    apps: HashMap<ConnectorId, Surfaces>,
}

impl ConnectSupport {
    /// Connect's catalog, as far as it is known.
    pub(crate) fn new(entries: impl IntoIterator<Item = (ConnectorId, Surfaces)>) -> Self {
        Self {
            apps: entries.into_iter().collect(),
        }
    }

    /// What Connect provides for `app`, or `None` when it does not serve it.
    pub(crate) fn surfaces(&self, app: &ConnectorId) -> Option<Surfaces> {
        self.apps.get(app).copied()
    }
}
