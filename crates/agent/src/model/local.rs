//! A model server the operator runs themselves: Ollama, `mlx_lm.server`, LM
//! Studio, llama.cpp, vLLM, or any other OpenAI-compatible Chat Completions
//! endpoint.
//!
//! It is the `local` provider of the router: ids `local/<model>` route to it.
//! With a model name configured too, it also stands in for every other
//! provider: [`ModelRouter::route`](super::router::ModelRouter::route)
//! rewrites each id to `local/<model>`, so product tiers and background
//! features that name Claude, GPT or Gemini are served by it. All of it is
//! optional configuration; nothing here is read or registered when the base
//! URL is unset.
//!
//! The configuration is operator-owned process environment, not Doppler, so
//! the three variables are optional and need no Doppler key.

use std::fmt;

use macro_env_var::maybe_env_var;

use crate::AgentError;

/// Provider segment the local server is registered under (`local/<model>`).
pub(crate) const LOCAL_PROVIDER: &str = "local";

/// Bearer sent when no key is configured. Most local servers take no key but
/// the client requires one to build.
const KEYLESS_BEARER: &str = "local";

maybe_env_var! {
    /// The optional local model server settings. A blank value is unset.
    struct LocalLlmEnv {
        /// Base URL of the OpenAI-compatible server, including its `/v1`
        /// path, e.g. `http://host.docker.internal:11434/v1` for Ollama.
        LocalLlmBaseUrl,
        /// Bearer token the server requires, if any.
        LocalLlmApiKey,
        /// The model every request is served by. Unset leaves routing as it
        /// is, so only `local/<model>` ids reach the server.
        LocalLlmModel,
    }
}

/// Where a local model server is and which model replaces the others.
#[derive(Clone, PartialEq, Eq)]
pub struct LocalLlm {
    base_url: String,
    api_key: String,
    model: Option<String>,
}

impl fmt::Debug for LocalLlm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalLlm")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .finish()
    }
}

impl LocalLlm {
    /// The local server configured in the environment, or `None` when
    /// `LOCAL_LLM_BASE_URL` is unset.
    ///
    /// A model name without a base URL is a mistake, not an absent server:
    /// every request would otherwise quietly go to the hosted providers.
    pub fn from_env() -> Result<Option<Self>, AgentError> {
        let env = LocalLlmEnv::new();
        Self::from_values(
            env.local_llm_base_url.as_ref().and_then(|v| v.value()),
            env.local_llm_api_key.as_ref().and_then(|v| v.value()),
            env.local_llm_model.as_ref().and_then(|v| v.value()),
        )
    }

    /// The same, from already-read values; blank ones are unset.
    pub(crate) fn from_values(
        base_url: Option<&str>,
        api_key: Option<&str>,
        model: Option<&str>,
    ) -> Result<Option<Self>, AgentError> {
        let present = |value: Option<&str>| {
            value
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let (base_url, api_key, model) = (present(base_url), present(api_key), present(model));
        let Some(base_url) = base_url else {
            if model.is_some() {
                return Err(AgentError::Other(anyhow::anyhow!(
                    "LOCAL_LLM_MODEL is set but LOCAL_LLM_BASE_URL is not"
                )));
            }
            return Ok(None);
        };
        Ok(Some(Self {
            base_url,
            api_key: api_key.unwrap_or_else(|| KEYLESS_BEARER.to_owned()),
            model,
        }))
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    /// The model that replaces every requested one, if configured.
    pub(crate) fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
}

#[cfg(test)]
mod test;
