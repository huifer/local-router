//! Routing: who answers, and in what order.

use std::collections::BTreeMap;

use local_router::config::{Config, Group, Provider, ProviderKey, Routing, Stays};
use local_router::gateway::route::Router;

fn provider(id: &str, models: &[&str], keys: usize) -> Provider {
    Provider {
        id: id.to_string(),
        name: id.to_string(),
        kind: "custom".into(),
        enabled: true,
        protocol: local_router::protocol::ir::Protocol::OpenAiChat,
        base_url: Some(format!("http://127.0.0.1:9/{id}/v1")),
        anthropic_base_url: None,
        responses_base_url: None,
        gemini_base_url: None,
        keys: (0..keys)
            .map(|_| ProviderKey::new(format!("sk-{id}")))
            .collect(),
        models: models.iter().map(|m| m.to_string()).collect(),
        model_map: Default::default(),
        auto_include: Vec::new(),
        catalog: None,
        headers: Default::default(),
        user_agent: None,
        forward_headers: Vec::new(),
        remove_headers: Vec::new(),
        header_order: Vec::new(),
        oauth: None,
        proxy: None,
        group_only: false,
        auth: None,
        chat_path: None,
        stream_only: false,
        hold_when_exhausted: false,
        max_concurrency: None,
        rpm: None,
        tpm: None,
        timeout_secs: None,
        connect_timeout_secs: None,
        first_byte_timeout_secs: None,
        schedule: Vec::new(),
        media_models: Vec::new(),
        media_paths: None,
    }
}

fn config_with(providers: Vec<Provider>, groups: Vec<Group>) -> Config {
    Config {
        providers,
        groups,
        settings: Default::default(),
        caller_keys: Vec::new(),
        ..Default::default()
    }
}

fn group(id: &str, models: &[&str], routing: Routing) -> Group {
    Group {
        id: id.to_string(),
        name: id.to_string(),
        models: models.iter().map(|m| m.to_string()).collect(),
        routing,
        weights: BTreeMap::new(),
        stays: Stays::Off,
        listed: true,
        alias: None,
        note: None,
        scenarios: Default::default(),
        max_price: None,
    }
}

#[tokio::test]
async fn direct_model_lists_every_key() {
    let config = config_with(vec![provider("a", &["m1"], 3)], vec![]);
    let router = Router::new();
    let plan = router.plan(&config, "a/m1", None, true).await.unwrap();
    assert_eq!(plan.targets.len(), 3);
    assert_eq!(plan.targets[0].model, "m1");
    assert_eq!(plan.group_id, None);
}

#[test]
fn split_model_needs_both_halves() {
    assert_eq!(Config::split_model("a/m1"), Some(("a", "m1")));
    assert_eq!(Config::split_model("am1"), None);
    assert_eq!(Config::split_model("a/"), None);
}

#[tokio::test]
async fn a_bare_model_id_resolves_to_the_only_provider_serving_it() {
    // A gateway in front of us may strip the prefix before forwarding.
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![],
    );
    let router = Router::new();
    let plan = router.plan(&config, "m2", None, true).await.unwrap();
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(plan.targets[0].provider_id, "b");
    assert_eq!(plan.targets[0].model, "m2");
}

#[tokio::test]
async fn a_bare_model_id_served_twice_is_refused() {
    // Two providers offer the same name: picking one would be a coin flip.
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m1"], 1)],
        vec![],
    );
    let router = Router::new();
    let err = router.plan(&config, "m1", None, true).await.unwrap_err();
    assert!(err.to_string().contains("both a and b"));
    // The prefixed form still works and picks the one that was asked for.
    let plan = router.plan(&config, "b/m1", None, true).await.unwrap();
    assert_eq!(plan.targets[0].provider_id, "b");
}

#[tokio::test]
async fn a_bare_model_id_nobody_serves_is_an_error() {
    let config = config_with(vec![provider("a", &["m1"], 1)], vec![]);
    let router = Router::new();
    let err = router.plan(&config, "nope", None, true).await.unwrap_err();
    assert!(err.to_string().contains("not a model id"));
}

#[tokio::test]
async fn a_nested_gateway_can_forward_bare_model_ids() {
    // The inner gateway receives what the outer one forwarded: a bare name.
    let inner = config_with(vec![provider("claude", &["claude-sonnet-4-5"], 1)], vec![]);
    let router = Router::new();
    let plan = router
        .plan(&inner, "claude-sonnet-4-5", None, true)
        .await
        .unwrap();
    assert_eq!(plan.targets[0].provider_id, "claude");
    // And what it sends upstream is the vendor's own id, not the prefix.
    assert_eq!(plan.targets[0].model, "claude-sonnet-4-5");
}

#[tokio::test]
async fn order_group_tries_members_in_order() {
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![group("g", &["a/m1", "b/m2"], Routing::Order)],
    );
    let router = Router::new();
    let plan = router.plan(&config, "group/g", None, true).await.unwrap();
    let models: Vec<&str> = plan.targets.iter().map(|t| t.model.as_str()).collect();
    assert_eq!(models, vec!["m1", "m2"]);
}

#[tokio::test]
async fn rotate_group_turns_each_request() {
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![group("g", &["a/m1", "b/m2"], Routing::Rotate)],
    );
    let router = Router::new();
    let first = router.plan(&config, "group/g", None, true).await.unwrap();
    let second = router.plan(&config, "group/g", None, true).await.unwrap();
    assert_eq!(first.targets[0].model, "m1");
    assert_eq!(second.targets[0].model, "m2");
}

#[tokio::test]
async fn usage_group_puts_the_least_used_first() {
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![group("g", &["a/m1", "b/m2"], Routing::Usage)],
    );
    let router = Router::new();
    // Warm b up.
    router.note_used("b/m2").await;
    router.note_used("b/m2").await;
    let plan = router.plan(&config, "group/g", None, true).await.unwrap();
    assert_eq!(plan.targets[0].model, "m1");
}

#[tokio::test]
async fn nested_groups_flatten() {
    let config = config_with(
        vec![
            provider("a", &["m1"], 1),
            provider("b", &["m2"], 1),
            provider("c", &["m3"], 1),
        ],
        vec![
            group("inner", &["a/m1", "b/m2"], Routing::Order),
            group("outer", &["group/inner", "c/m3"], Routing::Order),
        ],
    );
    let router = Router::new();
    let plan = router
        .plan(&config, "group/outer", None, true)
        .await
        .unwrap();
    let models: Vec<&str> = plan.targets.iter().map(|t| t.model.as_str()).collect();
    assert_eq!(models, vec!["m1", "m2", "m3"]);
}

#[tokio::test]
async fn a_cycle_in_nested_groups_does_not_loop() {
    let config = config_with(
        vec![provider("a", &["m1"], 1)],
        vec![
            group("one", &["group/two", "a/m1"], Routing::Order),
            group("two", &["group/one"], Routing::Order),
        ],
    );
    let router = Router::new();
    let plan = router.plan(&config, "group/one", None, true).await.unwrap();
    assert_eq!(plan.targets.len(), 1);
}

#[tokio::test]
async fn session_stays_with_its_target() {
    let mut g = group("g", &["a/m1", "b/m2"], Routing::Order);
    g.stays = Stays::Session;
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![g],
    );
    let router = Router::new();
    let first = router
        .plan(&config, "group/g", Some("s1"), true)
        .await
        .unwrap();
    assert_eq!(first.targets[0].model, "m1");
    // The second member answered instead; the conversation follows it.
    router.note_affinity("s1", &first.targets[1].key_id).await;
    let second = router
        .plan(&config, "group/g", Some("s1"), true)
        .await
        .unwrap();
    assert_eq!(second.targets[0].key_id, first.targets[1].key_id);
    // Another session is not tied to it.
    let other = router
        .plan(&config, "group/g", Some("s2"), true)
        .await
        .unwrap();
    assert_eq!(other.targets[0].key_id, first.targets[0].key_id);
}

#[tokio::test]
async fn unknown_names_are_errors() {
    let config = config_with(vec![provider("a", &["m1"], 1)], vec![]);
    let router = Router::new();
    assert!(router
        .plan(&config, "group/nope", None, true)
        .await
        .is_err());
    assert!(router.plan(&config, "a/m9", None, true).await.is_err());
    assert!(router.plan(&config, "z/m1", None, true).await.is_err());
    assert!(router
        .plan(&config, "just-a-name", None, true)
        .await
        .is_err());
}

#[test]
fn weights_shape_the_order() {
    use local_router::gateway::route::weighted_order;
    let members = vec!["a".to_string(), "b".to_string()];
    let mut weights = BTreeMap::new();
    weights.insert("a".to_string(), 3);
    weights.insert("b".to_string(), 1);
    let out = weighted_order(&members, &weights, None);
    // a takes three for every one b takes.
    assert_eq!(out.iter().filter(|m| *m == "a").count(), 3);
    assert_eq!(out.iter().filter(|m| *m == "b").count(), 1);
    assert_eq!(out[0], "a");
    assert_eq!(out[3], "b");
}

#[tokio::test]
async fn strict_stays_refuse_to_fail_over() {
    let mut g = group("g", &["a/m1", "b/m2"], Routing::Order);
    g.stays = Stays::Strict;
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![g],
    );
    let router = Router::new();
    let first = router
        .plan(&config, "group/g", Some("s1"), true)
        .await
        .unwrap();
    // The session's target went away; strict mode answers with that target
    // alone — no failover list behind it.
    router.note_affinity("s1", &first.targets[1].key_id).await;
    let second = router
        .plan(&config, "group/g", Some("s1"), true)
        .await
        .unwrap();
    assert_eq!(second.targets.len(), 1);
    assert_eq!(second.targets[0].key_id, first.targets[1].key_id);
    assert!(second.reason.contains("strict"));

    // A session whose target never existed cannot be served at all.
    router.note_affinity("s2", "nonexistent-key").await;
    let err = router
        .plan(&config, "group/g", Some("s2"), true)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("strict"));
    // A session with no affinity plans the full list as usual.
    let third = router
        .plan(&config, "group/g", Some("s3"), true)
        .await
        .unwrap();
    assert_eq!(third.targets.len(), 2);
}

#[tokio::test]
async fn turn_stays_pick_again_on_a_fresh_exchange() {
    let mut g = group("g", &["a/m1", "b/m2"], Routing::Rotate);
    g.stays = Stays::Turn;
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![g],
    );
    let router = Router::new();
    // First turn: order lands on a, and the affinity is recorded.
    let first = router
        .plan(&config, "group/g", Some("s1"), true)
        .await
        .unwrap();
    assert_eq!(first.targets[0].model, "m1");
    // The turn's second member answered; the continuing exchange follows it
    // (has_history keeps the affinity bound).
    router.note_affinity("s1", &first.targets[1].key_id).await;
    let continued = router
        .plan(&config, "group/g", Some("s1"), true)
        .await
        .unwrap();
    assert_eq!(continued.targets[0].key_id, first.targets[1].key_id);
    // A fresh exchange (no history) clears the affinity: order picks a again.
    let fresh = router
        .plan(&config, "group/g", Some("s1"), false)
        .await
        .unwrap();
    assert_eq!(
        fresh.targets[0].model, "m1",
        "a fresh exchange must pick again under turn stays"
    );
    // A session-scoped group keeps its affinity across a fresh exchange.
    let mut gs = group("gs", &["a/m1", "b/m2"], Routing::Rotate);
    gs.stays = Stays::Session;
    let config_s = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![gs],
    );
    let router_s = Router::new();
    let first_s = router_s
        .plan(&config_s, "group/gs", Some("s9"), true)
        .await
        .unwrap();
    router_s
        .note_affinity("s9", &first_s.targets[0].key_id)
        .await;
    let fresh_s = router_s
        .plan(&config_s, "group/gs", Some("s9"), false)
        .await
        .unwrap();
    assert_eq!(fresh_s.targets[0].key_id, first_s.targets[0].key_id);
}

// ------------------------------------------------------------- aliases --

#[tokio::test]
async fn a_group_alias_routes_like_group_id() {
    let mut g = group("auto", &["a/m1", "b/m2"], Routing::Order);
    g.alias = Some("local-router/auto".into());
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![g],
    );
    let router = Router::new();
    let plan = router
        .plan(&config, "local-router/auto", None, true)
        .await
        .unwrap();
    assert_eq!(plan.group_id.as_deref(), Some("auto"));
    assert_eq!(plan.targets.len(), 2);
    // The canonical name keeps working.
    let canonical = router
        .plan(&config, "group/auto", None, true)
        .await
        .unwrap();
    assert_eq!(canonical.targets.len(), plan.targets.len());
}

#[tokio::test]
async fn a_real_provider_model_wins_over_an_alias() {
    let mut g = group("auto", &["b/m2"], Routing::Order);
    // The alias head names a provider that really serves that model.
    g.alias = Some("a/m1".into());
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![g],
    );
    let router = Router::new();
    let plan = router.plan(&config, "a/m1", None, true).await.unwrap();
    assert_eq!(plan.group_id, None);
    assert_eq!(plan.targets[0].model, "m1");
}

#[test]
fn alias_validation_rejects_bad_shapes_and_collisions() {
    let mut config = config_with(vec![provider("a", &["m1"], 1)], vec![]);
    let mut g = group("auto", &["a/m1"], Routing::Order);
    // Not provider/model shaped.
    g.alias = Some("plain-name".into());
    config.groups.push(g.clone());
    assert!(config.validate().is_err());
    config.groups.clear();
    // Reserved prefix.
    g.alias = Some("group/auto".into());
    config.groups.push(g.clone());
    assert!(config.validate().is_err());
    config.groups.clear();
    // Collides with a model the named provider really serves.
    g.alias = Some("a/m1".into());
    config.groups.push(g.clone());
    assert!(config.validate().is_err());
    config.groups.clear();
    // A fresh head is fine.
    g.alias = Some("local-router/auto".into());
    config.groups.push(g);
    assert!(config.validate().is_ok());
}

#[test]
fn duplicate_aliases_are_refused() {
    let mut g1 = group("auto", &["a/m1"], Routing::Order);
    g1.alias = Some("local-router/auto".into());
    let mut g2 = group("spare", &["a/m1"], Routing::Order);
    g2.alias = Some("local-router/auto".into());
    let config = config_with(vec![provider("a", &["m1"], 1)], vec![g1, g2]);
    assert!(config.validate().is_err());
}

#[test]
fn a_listed_alias_appears_in_the_model_catalog() {
    let mut g = group("auto", &["a/m1"], Routing::Order);
    g.alias = Some("local-router/auto".into());
    let config = config_with(vec![provider("a", &["m1"], 1)], vec![g]);
    let ids = config.model_ids();
    assert!(ids.contains(&"group/auto".to_string()));
    assert!(ids.contains(&"local-router/auto".to_string()));
}

// --------------------------------------------------------- group_only --
// The config field and console always advertised "only reachable through
// a routing group". Enforced in both the direct plan and the catalog.

#[tokio::test]
async fn group_only_refuses_a_direct_model_name() {
    let mut hidden = provider("hidden", &["m1"], 1);
    hidden.group_only = true;
    let config = config_with(vec![hidden], vec![]);
    let router = Router::new();
    let err = router
        .plan(&config, "hidden/m1", None, true)
        .await
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("only reachable through a routing group"));
    // A bare name must not sneak past the restriction either.
    let err = router.plan(&config, "m1", None, true).await.unwrap_err();
    assert!(err.to_string().contains("not a model id"));
}

#[tokio::test]
async fn group_only_still_answers_through_its_group() {
    let mut hidden = provider("hidden", &["m1"], 1);
    hidden.group_only = true;
    let config = config_with(
        vec![hidden],
        vec![group("auto", &["hidden/m1"], Routing::Order)],
    );
    let router = Router::new();
    let plan = router
        .plan(&config, "group/auto", None, true)
        .await
        .unwrap();
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(plan.targets[0].provider_id, "hidden");
}

#[test]
fn group_only_is_hidden_from_the_model_catalog() {
    let mut hidden = provider("hidden", &["m1"], 1);
    hidden.group_only = true;
    let config = config_with(vec![hidden, provider("open", &["m2"], 1)], vec![]);
    let ids = config.model_ids();
    assert!(!ids.iter().any(|id| id.starts_with("hidden/")));
    assert!(ids.contains(&"open/m2".to_string()));
}

// ------------------------------------------------------------- smart --
// Smart is not "least used": it prefers the member that has actually been
// fast and healthy, and pushes a failing one to the back.

#[tokio::test]
async fn smart_prefers_the_faster_healthy_member() {
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![group("g", &["a/m1", "b/m2"], Routing::Smart)],
    );
    let router = Router::new();
    router.note_outcome("a/m1", 2_000, true).await;
    router.note_outcome("b/m2", 200, true).await;
    router.note_outcome("b/m2", 200, true).await;
    let plan = router.plan(&config, "group/g", None, true).await.unwrap();
    assert_eq!(plan.targets[0].provider_id, "b");
}

#[tokio::test]
async fn smart_pushes_a_failing_member_to_the_back() {
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![group("g", &["a/m1", "b/m2"], Routing::Smart)],
    );
    let router = Router::new();
    router.note_outcome("a/m1", 100, true).await;
    router.note_outcome("b/m2", 100, false).await;
    assert!(router.score_of("b/m2").await >= 3_000);
    let plan = router.plan(&config, "group/g", None, true).await.unwrap();
    assert_eq!(plan.targets[0].provider_id, "a");
    // A success forgives one failure, so recovery is possible.
    router.note_outcome("b/m2", 100, true).await;
    assert!(router.score_of("b/m2").await < 3_000);
}

#[tokio::test]
async fn an_untried_member_gets_a_first_chance() {
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![group("g", &["a/m1", "b/m2"], Routing::Smart)],
    );
    let router = Router::new();
    router.note_outcome("a/m1", 5_000, true).await;
    let plan = router.plan(&config, "group/g", None, true).await.unwrap();
    assert_eq!(plan.targets[0].provider_id, "b");
}

// ----------------------------------------------------------- model_map --
// A public name and the vendor's real id are different things.

#[tokio::test]
async fn model_map_sends_the_vendor_its_own_id() {
    let mut p = provider("relay", &["clean-name"], 1);
    p.model_map
        .insert("clean-name".into(), "opaque-vendor-id-42".into());
    let config = config_with(vec![p], vec![]);
    let router = Router::new();
    let plan = router
        .plan(&config, "relay/clean-name", None, true)
        .await
        .unwrap();
    assert_eq!(plan.targets[0].model, "opaque-vendor-id-42");
    assert_eq!(plan.targets[0].asked, "relay/clean-name");
    // The public name is still the one in the catalog.
    assert!(config.model_ids().contains(&"relay/clean-name".to_string()));
}

#[tokio::test]
async fn model_map_applies_to_group_members_too() {
    let mut p = provider("relay", &["clean-name"], 1);
    p.model_map.insert("clean-name".into(), "real-id".into());
    let config = config_with(
        vec![p],
        vec![group("auto", &["relay/clean-name"], Routing::Order)],
    );
    let router = Router::new();
    let plan = router
        .plan(&config, "group/auto", None, true)
        .await
        .unwrap();
    assert_eq!(plan.targets[0].model, "real-id");
}

#[test]
fn model_map_validation_refuses_empty_and_prefixed_keys() {
    let mut p = provider("relay", &["m"], 1);
    p.model_map.insert("m".into(), "".into());
    let config = config_with(vec![p.clone()], vec![]);
    assert!(config.validate().is_err());
    p.model_map.clear();
    p.model_map.insert("relay/m".into(), "real".into());
    let config = config_with(vec![p], vec![]);
    assert!(config.validate().is_err());
}

// ----------------------------------------------------------- scenarios --
// One group serves different request shapes with different members.

#[test]
fn scenarios_pick_by_specificity() {
    use local_router::config::{choose_scenario, Scenarios};
    let sc = Scenarios {
        background: Some("group/cheap".into()),
        think: Some("group/reason".into()),
        long_context: Some("group/long".into()),
        long_context_tokens: Some(50_000),
    };
    // A huge conversation wins even when it also wants reasoning.
    assert_eq!(
        choose_scenario(&sc, true, true, None, 80_000),
        Some("long_context")
    );
    assert_eq!(
        choose_scenario(&sc, true, true, None, 10_000),
        Some("think")
    );
    // A cheap throwaway call: no tools, tiny output budget.
    assert_eq!(
        choose_scenario(&sc, false, false, Some(256), 1_000),
        Some("background")
    );
    // An agentic call with tools is never background.
    assert_eq!(choose_scenario(&sc, false, true, Some(256), 1_000), None);
    // No configured scenario, no match.
    assert_eq!(
        choose_scenario(&Scenarios::default(), true, true, None, 80_000),
        None
    );
}

#[tokio::test]
async fn a_scenario_routes_the_group_elsewhere() {
    let mut auto = group("auto", &["a/m1"], Routing::Order);
    auto.scenarios.background = Some("group/cheap".into());
    let cheap = group("cheap", &["b/m2"], Routing::Order);
    let config = config_with(
        vec![provider("a", &["m1"], 1), provider("b", &["m2"], 1)],
        vec![auto, cheap],
    );
    let router = Router::new();
    // The default member answers a normal request.
    let plan = router
        .plan(&config, "group/auto", None, true)
        .await
        .unwrap();
    assert_eq!(plan.targets[0].provider_id, "a");
    // The scenario target answers when asked for directly (the gateway
    // resolves the scenario before the plan).
    let plan = router
        .plan(&config, "group/cheap", None, true)
        .await
        .unwrap();
    assert_eq!(plan.targets[0].provider_id, "b");
}

// ---------------------------------------------------------- price cap --
// A group can refuse members that cost more than its ceiling. Unknown
// prices are kept: no price on file is not proof of an expensive model.

#[tokio::test]
async fn a_price_ceiling_drops_expensive_members() {
    let mut g = group(
        "budget",
        &["cheap/gpt-4o-mini", "rich/claude-opus-4-5"],
        Routing::Order,
    );
    g.max_price = Some(1.0); // $1/M input+output
    let config = config_with(
        vec![
            provider("cheap", &["gpt-4o-mini"], 1),
            provider("rich", &["claude-opus-4-5"], 1),
        ],
        vec![g],
    );
    let router = Router::new();
    let plan = router
        .plan(&config, "group/budget", None, true)
        .await
        .unwrap();
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(plan.targets[0].provider_id, "cheap");
}

#[tokio::test]
async fn unpriced_members_survive_a_price_ceiling() {
    let mut g = group("budget", &["custom/unknown"], Routing::Order);
    g.max_price = Some(0.01);
    let config = config_with(vec![provider("custom", &["unknown"], 1)], vec![g]);
    let router = Router::new();
    let plan = router
        .plan(&config, "group/budget", None, true)
        .await
        .unwrap();
    assert_eq!(plan.targets.len(), 1);
}
