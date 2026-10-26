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
        auto_include: Vec::new(),
        catalog: None,
        headers: BTreeMap::new(),
        user_agent: None,
        proxy: None,
        group_only: false,
        auth: None,
        chat_path: None,
        stream_only: false,
        hold_when_exhausted: false,
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
        note: None,
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
