use super::resolve_local_runtime_key;
use crate::openhuman::config::Config;

#[test]
fn omlx_key_falls_back_to_local_ai_api_key() {
    let mut config = Config::default();
    config.local_ai.api_key = Some("  sk-omlx-list  ".into());
    assert_eq!(
        resolve_local_runtime_key("omlx", String::new(), &config),
        "sk-omlx-list"
    );
}

#[test]
fn looked_up_key_wins_over_local_ai() {
    let mut config = Config::default();
    config.local_ai.api_key = Some("sk-local".into());
    assert_eq!(
        resolve_local_runtime_key("omlx", "from-profiles".into(), &config),
        "from-profiles"
    );
}

#[test]
fn non_omlx_slug_does_not_fall_back() {
    let mut config = Config::default();
    config.local_ai.api_key = Some("sk-local".into());
    assert_eq!(
        resolve_local_runtime_key("ollama", String::new(), &config),
        ""
    );
}

use super::{config_declared_models, endpoint_is_http, list_configured_models_from_config};
use crate::openhuman::config::schema::cloud_providers::{AuthStyle, CloudProviderCreds};

#[test]
fn only_http_endpoints_are_probed() {
    assert!(endpoint_is_http("https://api.openai.com/v1"));
    assert!(endpoint_is_http("http://localhost:11434/v1"));
    assert!(endpoint_is_http("  HTTPS://api.openai.com/v1  "));
    assert!(!endpoint_is_http("cli://claude-code"));
    assert!(!endpoint_is_http(""));
    assert!(!endpoint_is_http("claude-code"));
}

fn claude_code_entry() -> CloudProviderCreds {
    CloudProviderCreds {
        id: "p_claude_code_1".to_string(),
        slug: "claude-code".to_string(),
        label: "Claude Code".to_string(),
        endpoint: "cli://claude-code".to_string(),
        auth_style: AuthStyle::None,
        ..CloudProviderCreds::default()
    }
}

/// `cli://claude-code` is the endpoint the settings panel writes for the
/// claude-code provider. Before this, the listing built `cli://claude-code/models`,
/// which reqwest refuses to send — the RPC failed and logged at ERR on every
/// model listing. It must now succeed with an empty listing.
#[tokio::test]
async fn cli_endpoint_listing_succeeds_without_an_http_probe() {
    let mut config = Config::default();
    config.cloud_providers = vec![claude_code_entry()];

    let outcome = list_configured_models_from_config("claude-code", &config)
        .await
        .expect("a cli:// provider must not fail the listing");
    assert_eq!(
        outcome.value["models"],
        serde_json::json!([]),
        "nothing in config declares a claude-code model, so the listing is empty"
    );
}

/// A model the config does declare rides through instead of being dropped.
#[tokio::test]
async fn cli_endpoint_listing_returns_config_declared_models() {
    let mut entry = claude_code_entry();
    entry.default_model = Some("  sonnet  ".to_string());
    let mut config = Config::default();
    config.cloud_providers = vec![entry.clone()];

    let declared = config_declared_models(&entry);
    assert_eq!(declared.len(), 1);
    assert_eq!(declared[0].id, "sonnet");

    let outcome = list_configured_models_from_config("claude-code", &config)
        .await
        .expect("a cli:// provider must not fail the listing");
    assert_eq!(outcome.value["models"][0]["id"], "sonnet");
}
