//! local-router: a local gateway that gives every coding agent every model.
//!
//! The web page and the gateway are the same process. `local-router` starts both;
//! `local-router serve` starts the gateway alone.

use local_router::{config, gateway, presets, pricing, protocol, store, web, workbuddy};

use std::sync::Arc;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use serde_json::Value;

use crate::config::{AppState, Config, ProviderKey, Routing, Stays, Store};

#[derive(Parser, Debug)]
#[command(
    name = "local-router",
    version,
    about = "A local gateway that gives every coding agent every model",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start the gateway, and the web page with it.
    Serve(ServeArgs),
    /// Start only the web page.
    Web(WebArgs),
    /// Manage the providers.
    #[command(subcommand)]
    Provider(ProviderCmd),
    /// List the models the gateway serves.
    Models,
    /// Manage the routing groups.
    #[command(subcommand)]
    Group(GroupCmd),
    /// Show what is configured.
    Ls,
    /// Show the recorded usage.
    Usage(UsageArgs),
    /// Point Claude Code at a model.
    Claude(AgentModelArgs),
    /// Point Codex at a model.
    Codex(AgentModelArgs),
    /// Point Gemini CLI at a model.
    Gemini(AgentModelArgs),
    /// Point OpenCode at a model.
    Opencode(AgentModelArgs),
    /// Check that everything is in order.
    Doctor,
    /// The terminal dashboard: live providers, traffic and spend.
    Dashboard,
    /// Manage the background daemon (start, stop, status, logs).
    #[command(subcommand)]
    Daemon(DaemonCmd),
    /// WorkBuddy: the local login session, its credits and its models.
    #[command(name = "workbuddy", subcommand)]
    WorkBuddy(WorkBuddyCmd),
    /// Show the price table the gateway bills with.
    Prices,
}

#[derive(Subcommand, Debug)]
enum DaemonCmd {
    /// Start the daemon in the background (gateway + console, one process).
    Start {
        /// The gateway port.
        #[arg(long, env = "LOCAL_ROUTER_GATEWAY_PORT", default_value_t = 3425)]
        port: u16,
        /// The console port (0 turns it off).
        #[arg(long, env = "LOCAL_ROUTER_WEB_PORT", default_value_t = 3426)]
        web_port: u16,
        /// The address the gateway binds to.
        #[arg(long, env = "LOCAL_ROUTER_GATEWAY_ADDR", default_value = "127.0.0.1")]
        addr: String,
        /// The address the console binds to.
        #[arg(long, env = "LOCAL_ROUTER_WEB_ADDR", default_value = "127.0.0.1")]
        web_addr: Option<String>,
        /// Bind on the local network (both addresses become 0.0.0.0).
        #[arg(long)]
        lan: bool,
        /// Wait until the ports answer, then return.
        #[arg(long, default_value_t = true)]
        wait: bool,
    },
    /// Stop the running daemon.
    Stop,
    /// Whether the daemon is running, and where.
    Status,
    /// Follow the daemon's log.
    Logs {
        /// Only show the last N lines first.
        #[arg(long, default_value_t = 40)]
        lines: usize,
    },
}

#[derive(Subcommand, Debug)]
enum WorkBuddyCmd {
    /// Show whether the WorkBuddy app is signed in.
    Status,
    /// Show the credits on the account.
    Credits,
    /// List the models the account may use.
    Models,
    /// Claim the day's check-in now.
    Checkin,
    /// Sign in to WorkBuddy: print a link, wait for the browser, wire it up.
    Login,
}

impl Default for ServeArgs {
    fn default() -> Self {
        ServeArgs {
            port: env_port("LOCAL_ROUTER_GATEWAY_PORT", 3425),
            web_port: env_port("LOCAL_ROUTER_WEB_PORT", 3426),
            lan: false,
            addr: std::env::var("LOCAL_ROUTER_GATEWAY_ADDR")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "127.0.0.1".to_string()),
            web_addr: std::env::var("LOCAL_ROUTER_WEB_ADDR")
                .ok()
                .filter(|v| !v.trim().is_empty()),
        }
    }
}

#[derive(Args, Debug, Clone)]
struct ServeArgs {
    /// The port the gateway listens on.
    /// Defaults to LOCAL_ROUTER_GATEWAY_PORT, then the file, then 3425.
    #[arg(long, env = "LOCAL_ROUTER_GATEWAY_PORT", default_value_t = 3425)]
    port: u16,
    /// The port the web page listens on. Zero turns the page off.
    #[arg(long, env = "LOCAL_ROUTER_WEB_PORT", default_value_t = 3426)]
    web_port: u16,
    /// Bind on the local network, so other computers can use the gateway.
    #[arg(long)]
    lan: bool,
    /// The address the gateway binds to.
    #[arg(long, env = "LOCAL_ROUTER_GATEWAY_ADDR", default_value = "127.0.0.1")]
    addr: String,
    /// The address the console binds to. Defaults to the gateway address.
    #[arg(long, env = "LOCAL_ROUTER_WEB_ADDR")]
    web_addr: Option<String>,
}

#[derive(Args, Debug, Clone)]
struct WebArgs {
    /// The port the web page listens on.
    #[arg(long, env = "LOCAL_ROUTER_WEB_PORT", default_value_t = 3426)]
    port: u16,
    /// The port the gateway listens on.
    #[arg(long, env = "LOCAL_ROUTER_GATEWAY_PORT", default_value_t = 3425)]
    gateway_port: u16,
}

#[derive(Args, Debug, Clone)]
struct UsageArgs {
    /// Print csv instead of a table.
    #[arg(long)]
    csv: bool,
    /// Only show the last day: today, 7d or 30d.
    #[arg(long, default_value = "7d")]
    period: String,
}

#[derive(Args, Debug, Clone)]
struct AgentModelArgs {
    /// The model, as `provider/model` or `group/id`.
    model: String,
}

#[derive(Subcommand, Debug)]
enum ProviderCmd {
    /// Add a provider: a preset needs only a key.
    Add {
        /// The preset id, or `custom`.
        preset: String,
        /// The vendor key.
        #[arg(long)]
        key: Option<String>,
        /// A name, for a custom provider.
        #[arg(long)]
        name: Option<String>,
        /// The base url, for a custom provider.
        #[arg(long)]
        url: Option<String>,
        /// The models to expose, comma separated.
        #[arg(long, value_delimiter = ',')]
        models: Vec<String>,
    },
    /// List the providers.
    List,
    /// Remove a provider.
    Rm { id: String },
    /// Fetch the vendor's own list of models.
    Models { id: String },
    /// Set the models a provider exposes.
    Expose { id: String, models: Vec<String> },
    /// Set the models a provider offers for image and video work.
    Media { id: String, models: Vec<String> },
    /// Send a tiny request to a provider.
    Test { id: String },
    /// Add a key to a provider.
    Key { id: String, key: String },
}

#[derive(Subcommand, Debug)]
enum GroupCmd {
    /// Add a routing group.
    Add {
        /// The group's name.
        name: String,
        /// The members, comma separated.
        #[arg(long, value_delimiter = ',')]
        models: Vec<String>,
        /// How the group picks: smart, order, rotate, usage or weight.
        #[arg(long, default_value = "order")]
        routing: String,
        /// How long a conversation stays: auto, session, turn, off or strict.
        #[arg(long, default_value = "auto")]
        stays: String,
    },
    /// List the groups.
    List,
    /// Remove a group.
    Rm { id: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LOCAL_ROUTER_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Some(Command::Serve(args)) => serve(args).await,
        Some(Command::Web(args)) => web_only(args).await,
        Some(Command::Provider(cmd)) => provider_cmd(cmd).await,
        Some(Command::Models) => models().await,
        Some(Command::Group(cmd)) => group_cmd(cmd).await,
        Some(Command::Ls) => ls().await,
        Some(Command::Usage(args)) => usage(args).await,
        Some(Command::Claude(args)) => set_agent("claude", args).await,
        Some(Command::Codex(args)) => set_agent("codex", args).await,
        Some(Command::Gemini(args)) => set_agent("gemini", args).await,
        Some(Command::Opencode(args)) => set_agent("opencode", args).await,
        Some(Command::Doctor) => doctor().await,
        Some(Command::WorkBuddy(cmd)) => workbuddy_cmd(cmd).await,
        Some(Command::Prices) => prices().await,
        Some(Command::Dashboard) => local_router::dashboard::run().await,
        Some(Command::Daemon(cmd)) => daemon_cmd(cmd).await,
        None => serve(ServeArgs::default()).await,
    }
}

/// Read a port from the environment, falling back when absent or odd.
fn env_port(name: &str, fallback: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(fallback)
}

// ---------------------------------------------------------------- servers --

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let store = Arc::new(Store::new());
    let state = AppState::load(store).await?;
    let config = state.config.read().await;
    // Environment first (containers), then the command line (people).
    let mut settings = config.settings.clone().from_env();
    settings.gateway_port = args.port;
    settings.web_port = args.web_port;
    settings.gateway_addr = args.addr.clone();
    if let Some(web_addr) = &args.web_addr {
        settings.web_addr = web_addr.clone();
    }
    if args.lan {
        settings.gateway_addr = "0.0.0.0".to_string();
        settings.web_addr = "0.0.0.0".to_string();
    }
    drop(config);
    // Handlers that care — the console's remote guard — must see the real
    // bind address, not the file's.
    state.set_web_bind(&settings.web_addr).await;

    if settings.workbuddy_auto_checkin {
        start_auto_checkin();
    }

    let gateway = gateway::Gateway::new(state.clone());
    let gateway_addr = format!("{}:{}", settings.gateway_addr, settings.gateway_port);
    let gateway_router = gateway::router(gateway);
    let listener = tokio::net::TcpListener::bind(&gateway_addr)
        .await
        .with_context(|| format!("the gateway could not listen on {gateway_addr}"))?;

    let web_addr = format!("{}:{}", settings.web_addr, settings.web_port);
    let web_listener = if settings.web_port == 0 {
        None
    } else {
        Some(
            tokio::net::TcpListener::bind(&web_addr)
                .await
                .with_context(|| format!("the web page could not listen on {web_addr}"))?,
        )
    };

    println!("local-router {}", gateway::client::VERSION);
    println!("  gateway  http://{gateway_addr}");
    if web_listener.is_some() {
        println!("  web      http://{web_addr}");
    }
    println!("  press ctrl-c to stop");

    if let Some(listener) = web_listener {
        let web_router = web::router(state.clone());
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, web_router).await {
                eprintln!("the web page stopped: {err}");
            }
        });
    }

    axum::serve(listener, gateway_router)
        .with_graceful_shutdown(shutdown())
        .await
        .context("the gateway stopped")?;
    Ok(())
}

async fn web_only(args: WebArgs) -> anyhow::Result<()> {
    let store = Arc::new(Store::new());
    let state = AppState::load(store).await?;
    let settings = state.config.read().await.settings.clone().from_env();
    state.set_web_bind(&settings.web_addr).await;
    if settings.workbuddy_auto_checkin {
        start_auto_checkin();
    }

    let gateway = gateway::Gateway::new(state.clone());
    let gateway_addr = format!("{}:{}", settings.gateway_addr, args.gateway_port);
    let gateway_router = gateway::router(gateway);
    match tokio::net::TcpListener::bind(&gateway_addr).await {
        Ok(listener) => {
            println!("  gateway  http://{gateway_addr}");
            tokio::spawn(async move {
                let _ = axum::serve(listener, gateway_router).await;
            });
        }
        Err(err) => {
            eprintln!("  gateway  not started on {gateway_addr}: {err}");
        }
    }

    let web_addr = format!("{}:{}", settings.web_addr, args.port);
    let listener = tokio::net::TcpListener::bind(&web_addr)
        .await
        .with_context(|| format!("the web page could not listen on {web_addr}"))?;
    println!("  web      http://{web_addr}");
    println!("  press ctrl-c to stop");
    axum::serve(listener, web::router(state))
        .with_graceful_shutdown(shutdown())
        .await
        .context("the web page stopped")?;
    Ok(())
}

async fn shutdown() {
    // The daemon form has no terminal: it is stopped with SIGTERM.
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("the SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
    println!("\nstopping");
}

/// Claim the WorkBuddy account's daily check-in on a schedule, so the
/// day's credits land without anyone opening the app.
///
/// The first check waits a little: at boot the session file may not be
/// readable yet, and a failed attempt is simply retried later.
fn start_auto_checkin() {
    tokio::spawn(async move {
        let mut first = true;
        loop {
            let wait = if first {
                first = false;
                std::time::Duration::from_secs(30)
            } else {
                std::time::Duration::from_secs(30 * 60)
            };
            tokio::time::sleep(wait).await;

            // 未登录或登录态不可读：安静跳过
            let Some(status) = workbuddy::checkin_state().await else {
                continue;
            };
            if !status.active || status.checked_in_today {
                tracing::debug!("WorkBuddy 自动签到：今日无需处理");
                continue;
            }
            match workbuddy::claim_checkin().await {
                Ok(outcome) if outcome.granted => tracing::info!(
                    "WorkBuddy 自动签到完成：{}{}",
                    outcome
                        .credit
                        .map(|c| format!("+{c} credits "))
                        .unwrap_or_default(),
                    outcome.message
                ),
                Ok(outcome) if outcome.already => {
                    tracing::debug!("WorkBuddy 今日已签到：{}", outcome.message)
                }
                Ok(outcome) => tracing::warn!("WorkBuddy 签到未完成：{}", outcome.message),
                Err(err) => tracing::warn!("WorkBuddy 签到失败：{err}"),
            }
        }
    });
}

// -------------------------------------------------------------- providers --

fn load_config() -> anyhow::Result<Config> {
    Store::new().load()
}

fn save_config(config: &Config) -> anyhow::Result<()> {
    Store::new().save(config)
}

async fn provider_cmd(cmd: ProviderCmd) -> anyhow::Result<()> {
    let mut config = load_config()?;
    match cmd {
        ProviderCmd::Add {
            preset,
            key,
            name,
            url,
            models,
        } => {
            let provider = match presets::get(&preset) {
                Some(p) => {
                    let mut p = presets::to_provider(p, key.as_deref());
                    if let Some(url) = &url {
                        p.base_url = Some(url.trim_end_matches('/').to_string());
                    }
                    if !models.is_empty() {
                        p.models = models;
                    }
                    p
                }
                None => {
                    // Not a preset: a provider the user described by hand.
                    let name = name.unwrap_or_else(|| preset.clone());
                    let mut p = presets::custom_provider(
                        &name,
                        url.as_deref(),
                        None,
                        protocol::ir::Protocol::OpenAiChat,
                        key.as_deref(),
                        &models,
                    );
                    p.kind = "custom".to_string();
                    p
                }
            };
            let id = provider.id.clone();
            if config.providers.iter().any(|p| p.id == id) {
                anyhow::bail!("there is already a provider called {id}");
            }
            config.providers.push(provider);
            save_config(&config)?;
            println!("added the provider {id}");
        }
        ProviderCmd::List => {
            if config.providers.is_empty() {
                println!("no providers yet");
                return Ok(());
            }
            println!("{:<16} {:<22} {:<8} MODELS", "ID", "HOST", "KEYS");
            for p in &config.providers {
                let host = p
                    .base_url
                    .clone()
                    .unwrap_or_default()
                    .replace("https://", "")
                    .replace("http://", "")
                    .trim_end_matches('/')
                    .to_string();
                let keys = if p.uses_session() {
                    "登录态".to_string()
                } else {
                    p.keys.len().to_string()
                };
                let mut models = p.models.clone();
                models.extend(p.media_models.iter().cloned());
                println!(
                    "{:<16} {:<22} {:<8} {}",
                    p.id,
                    host,
                    keys,
                    models.join(", ")
                );
            }
        }
        ProviderCmd::Rm { id } => {
            let before = config.providers.len();
            config.providers.retain(|p| p.id != id);
            if config.providers.len() == before {
                anyhow::bail!("there is no provider called {id}");
            }
            for group in &mut config.groups {
                group.models.retain(|m| !m.starts_with(&format!("{id}/")));
            }
            save_config(&config)?;
            println!("removed the provider {id}");
        }
        ProviderCmd::Models { id } => {
            let Some(provider) = config.provider(&id) else {
                anyhow::bail!("there is no provider called {id}");
            };
            let models = fetch_models(provider).await?;
            println!("the vendor lists {} models:", models.len());
            for m in models {
                println!("  {m}");
            }
        }
        ProviderCmd::Expose { id, models } => {
            let count = {
                let Some(p) = config.provider_mut(&id) else {
                    anyhow::bail!("there is no provider called {id}");
                };
                p.models = models;
                p.models.len()
            };
            save_config(&config)?;
            println!("{id} now exposes {count} models");
        }
        ProviderCmd::Media { id, models } => {
            let count = {
                let Some(p) = config.provider_mut(&id) else {
                    anyhow::bail!("there is no provider called {id}");
                };
                p.media_models = models;
                p.media_models.len()
            };
            save_config(&config)?;
            println!("{id} now offers {count} media model(s)");
        }
        ProviderCmd::Test { id } => {
            let Some(provider) = config.provider(&id) else {
                anyhow::bail!("there is no provider called {id}");
            };
            let provider = provider.clone();
            match test_provider(&provider).await {
                Ok((latency, reply)) => {
                    println!("{id} answered in {latency} ms");
                    println!("  {reply}");
                }
                Err(err) => {
                    println!("{id} did not answer: {err}");
                }
            }
        }
        ProviderCmd::Key { id, key } => {
            let count = {
                let Some(p) = config.provider_mut(&id) else {
                    anyhow::bail!("there is no provider called {id}");
                };
                p.keys.push(ProviderKey::new(key));
                p.keys.len()
            };
            save_config(&config)?;
            println!("{id} now has {count} keys");
        }
    }
    Ok(())
}

async fn fetch_models(provider: &config::Provider) -> anyhow::Result<Vec<String>> {
    // A session-backed provider lists its models from its own account.
    if provider.uses_session() {
        let mut models = workbuddy::models().await?;
        models.sort();
        models.dedup();
        return Ok(models);
    }
    let base = provider
        .base_url
        .clone()
        .context("the provider has no url")?;
    let url = gateway::client::models_url(&base);
    let key = provider.enabled_keys().next().map(|k| k.key.clone());
    let http = reqwest::Client::builder()
        .user_agent(format!("local-router/{}", gateway::client::VERSION))
        .build()?;
    let mut req = http.get(&url);
    if let Some(k) = key {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("the vendor answered {status}: {text}");
    }
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let mut models: Vec<String> = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.get("id").and_then(serde_json::Value::as_str))
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    if models.is_empty() {
        models = value
            .get("models")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| {
                        i.get("name")
                            .or_else(|| i.get("id"))
                            .and_then(serde_json::Value::as_str)
                    })
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
    }
    models.sort();
    models.dedup();
    Ok(models)
}

async fn test_provider(provider: &config::Provider) -> anyhow::Result<(u128, String)> {
    let model = provider
        .models
        .first()
        .context("the provider exposes no model")?;
    // A session-backed provider has no stored key: the session is read
    // when the request goes out.
    let (headers, _token) = if provider.uses_session() {
        let session = workbuddy::session().await?;
        let headers = gateway::client::session_headers(provider.protocol, provider, &session);
        (headers, session.access_token.clone())
    } else {
        let key = provider
            .enabled_keys()
            .next()
            .context("the provider has no key")?;
        (
            gateway::client::auth_headers(provider.protocol, provider, &key.key, None),
            key.key.clone(),
        )
    };
    let base = provider
        .base_url
        .clone()
        .context("the provider has no url")?;
    let url = match &provider.chat_path {
        Some(path) => format!("{}{}", base.trim_end_matches('/'), path),
        None => gateway::client::url_for(&base, provider.protocol, Some(model), false),
    };
    let mut body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 16,
    });
    if provider.stream_only {
        body["stream"] = serde_json::json!(true);
        body["stream_options"] = serde_json::json!({"include_usage": true});
    }
    let http = reqwest::Client::builder()
        .user_agent(format!("local-router/{}", gateway::client::VERSION))
        .build()?;
    let mut req = http.post(&url).json(&body);
    for (name, value) in &headers {
        req = req.header(name.as_str(), value.as_str());
    }
    let started = std::time::Instant::now();
    let resp = req.send().await?;
    let status = resp.status();
    let latency = started.elapsed().as_millis();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("{}", text.chars().take(200).collect::<String>());
    }
    Ok((latency, text.chars().take(200).collect()))
}

// -------------------------------------------------------------- workbuddy --

async fn workbuddy_cmd(cmd: WorkBuddyCmd) -> anyhow::Result<()> {
    match cmd {
        WorkBuddyCmd::Status => {
            let path = workbuddy::session_file();
            println!("local-router {}", gateway::client::VERSION);
            match workbuddy::read_session_file() {
                Ok(session) => {
                    println!("  signed in   yes");
                    if !session.account.nickname.is_empty() {
                        println!(
                            "  account     {} ({})",
                            session.account.nickname, session.account.uid
                        );
                    }
                    match session.expires_at {
                        Some(at) => {
                            let left = (at - workbuddy::now_ms()) / 1000;
                            println!("  token until {} ({} h left)", format_time(at), left / 3600);
                        }
                        None => println!("  token until unknown"),
                    }
                    println!(
                        "  session     {}",
                        path.map(|p| p.display().to_string()).unwrap_or_default()
                    );
                }
                Err(err) => {
                    println!("  signed in   no");
                    println!("  {err}");
                }
            }
        }
        WorkBuddyCmd::Credits => {
            let credits = workbuddy::credits().await?;
            println!("credits left: {} / {}", credits.remaining, credits.capacity);
            for p in &credits.packages {
                println!(
                    "  {:<32} {}/{} {} used",
                    p.package_name, p.remaining, p.capacity, p.used
                );
            }
            if let Some(checkin) = &credits.checkin {
                println!(
                    "  check-in    {} (streak {} days, {} credits today)",
                    if checkin.checked_in_today {
                        "done"
                    } else {
                        "not done"
                    },
                    checkin.streak_days,
                    checkin.today_credit
                );
            }
        }
        WorkBuddyCmd::Models => {
            let models = workbuddy::models().await?;
            println!("the account may use {} models:", models.len());
            for m in models {
                println!("  {m}");
            }
        }
        WorkBuddyCmd::Login => login().await?,
        WorkBuddyCmd::Checkin => match workbuddy::claim_checkin().await {
            Ok(outcome) if outcome.granted => {
                println!(
                    "checked in — {}",
                    outcome
                        .credit
                        .map(|c| format!("+{c} credits"))
                        .unwrap_or_else(|| outcome.message.clone())
                );
            }
            Ok(outcome) if outcome.already => {
                println!("already checked in today — {}", outcome.message);
            }
            Ok(outcome) => println!("{}", outcome.message),
            Err(err) => {
                println!("the check-in did not go through: {err}");
                println!("sign in to WorkBuddy first, then try again");
            }
        },
    }
    Ok(())
}

/// Sign in to WorkBuddy from the terminal.
///
/// The vendor hands out a login link for a fresh state; the browser signs
/// in; the gateway polls until the tokens arrive. The tokens are stored in
/// local-router's own session file — WorkBuddy's desktop app and its login
/// file are left untouched.
async fn login() -> anyhow::Result<()> {
    let session = workbuddy::start_login().await?;
    println!("local-router {}", gateway::client::VERSION);
    println!("  open this link in a browser and sign in:");
    println!();
    println!("  {}", session.auth_url);
    println!();

    // Open the default browser for the person at the keyboard; on a
    // machine without one the link above still works.
    let _ = std::process::Command::new("open")
        .arg(&session.auth_url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    println!("  waiting for the sign-in (up to 5 minutes)…");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5 * 60);
    let result = loop {
        if std::time::Instant::now() > deadline {
            anyhow::bail!("the sign-in timed out; run `local-router workbuddy login` again");
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        match workbuddy::poll_login(&session.state).await {
            Ok(workbuddy::LoginPoll::Waiting) => continue,
            Ok(workbuddy::LoginPoll::Ready(result)) => break *result,
            Err(err) => return Err(err),
        }
    };

    // Who signed in, from the vendor's account endpoint.
    let account = workbuddy::login_account(&result.session, &session.state)
        .await
        .unwrap_or(Value::Null);
    let nickname = account
        .get("nickname")
        .or_else(|| account.pointer("/account/nickname"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    // Store the session in local-router's own file, next to the config.
    let path = workbuddy::store_session(&result.session, &account).await?;
    println!("  signed in   {nickname}");
    println!("  session     {}", path.display());
    println!(
        "  token until {}",
        result
            .session
            .expires_at
            .map(format_time)
            .unwrap_or_else(|| "unknown".to_string())
    );

    // Wire up the provider if it is not there yet.
    let mut config = load_config()?;
    if config.provider("workbuddy").is_none() {
        if let Some(preset) = presets::get("workbuddy") {
            config.providers.push(presets::to_provider(preset, None));
            save_config(&config)?;
            println!("  provider    workbuddy added (no key needed)");
        }
    }

    println!();
    println!("  done — try it: local-router workbuddy credits");
    Ok(())
}

// ---------------------------------------------------------------- daemon --

/// Where the daemon's bookkeeping lives.
fn daemon_dir() -> std::path::PathBuf {
    config::config_dir().join("daemon")
}

fn pid_file() -> std::path::PathBuf {
    daemon_dir().join("daemon.pid")
}

fn log_file() -> std::path::PathBuf {
    daemon_dir().join("daemon.log")
}

/// The pid of the running daemon, if there is one and it is alive.
fn running_pid() -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(pid_file())
        .ok()?
        .trim()
        .parse()
        .ok()?;
    // A pid file without a live process is a leftover: clean it up.
    let alive = std::path::Path::new(&format!("/proc/{pid}")).exists()
        || std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    if alive {
        Some(pid)
    } else {
        let _ = std::fs::remove_file(pid_file());
        None
    }
}

/// The arguments the daemon was started with, for `status` and `restart`.
fn save_start_args(args: &DaemonStart) -> anyhow::Result<()> {
    let path = daemon_dir().join("start.json");
    let body = serde_json::json!({
        "port": args.port,
        "web_port": args.web_port,
        "addr": args.addr,
        "web_addr": args.web_addr,
        "lan": args.lan,
    });
    std::fs::create_dir_all(daemon_dir())?;
    std::fs::write(path, serde_json::to_string_pretty(&body)?)?;
    Ok(())
}

async fn daemon_cmd(cmd: DaemonCmd) -> anyhow::Result<()> {
    match cmd {
        DaemonCmd::Start {
            port,
            web_port,
            addr,
            web_addr,
            lan,
            wait,
        } => {
            let args = DaemonStart {
                port,
                web_port,
                addr,
                web_addr,
                lan,
            };
            if let Some(pid) = running_pid() {
                println!("already running (pid {pid}) — stop it first: local-router daemon stop");
                return Ok(());
            }
            start_daemon(&args)?;
            if wait {
                // Wait until both ports answer, or say what went wrong.
                let gateway = format!("http://{}:{port}/health", args.addr);
                let web = if args.web_port == 0 {
                    None
                } else {
                    Some(format!(
                        "http://{}:{}/",
                        args.web_addr.clone().unwrap_or_else(|| args.addr.clone()),
                        args.web_port
                    ))
                };
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                loop {
                    let gateway_up = reqwest::get(&gateway)
                        .await
                        .map(|r| r.status().is_success())
                        .unwrap_or(false);
                    let web_up = match &web {
                        Some(url) => reqwest::get(url)
                            .await
                            .map(|r| r.status().is_success())
                            .unwrap_or(false),
                        None => true,
                    };
                    if gateway_up && web_up {
                        break;
                    }
                    if std::time::Instant::now() > deadline {
                        anyhow::bail!(
                            "the daemon did not come up in time; see {}",
                            log_file().display()
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
            println!(
                "daemon started  gateway http://{}:{}  console http://{}:{}",
                args.addr,
                args.port,
                args.web_addr.as_deref().unwrap_or(&args.addr),
                args.web_port
            );
            println!("logs: {}", log_file().display());
            Ok(())
        }
        DaemonCmd::Stop => {
            let Some(pid) = running_pid() else {
                println!("not running");
                let _ = std::fs::remove_file(pid_file());
                return Ok(());
            };
            // SIGTERM first (graceful); a stuck process gets SIGKILL.
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while running_pid() == Some(pid) && std::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            if running_pid() == Some(pid) {
                let _ = std::process::Command::new("kill")
                    .arg("-9")
                    .arg(pid.to_string())
                    .status();
                let _ = tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
            let _ = std::fs::remove_file(pid_file());
            println!("daemon stopped (was pid {pid})");
            Ok(())
        }
        DaemonCmd::Status => {
            match running_pid() {
                Some(pid) => {
                    let args = read_start_args().unwrap_or_default();
                    println!("running    yes (pid {pid})");
                    println!("gateway    http://{}:{}", args.addr, args.port);
                    if args.web_port > 0 {
                        println!(
                            "console    http://{}:{}",
                            args.web_addr.as_deref().unwrap_or(&args.addr),
                            args.web_port
                        );
                    }
                    println!("logs       {}", log_file().display());
                }
                None => println!("running    no"),
            }
            Ok(())
        }
        DaemonCmd::Logs { lines } => {
            let path = log_file();
            if !path.exists() {
                println!("no log yet at {}", path.display());
                return Ok(());
            }
            // `tail -f` is the honest way to follow a file.
            let status = std::process::Command::new("tail")
                .arg("-n")
                .arg(lines.to_string())
                .arg("-f")
                .arg(&path)
                .status()?;
            let _ = status;
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct DaemonStart {
    port: u16,
    web_port: u16,
    addr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    web_addr: Option<String>,
    #[serde(default)]
    lan: bool,
}

fn read_start_args() -> Option<DaemonStart> {
    let text = std::fs::read_to_string(daemon_dir().join("start.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Spawn this same binary as a detached background process.
fn start_daemon(args: &DaemonStart) -> anyhow::Result<()> {
    std::fs::create_dir_all(daemon_dir())?;
    let exe = std::env::current_exe()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file())?;

    let mut command = std::process::Command::new(exe);
    command
        .arg("serve")
        .arg("--port")
        .arg(args.port.to_string())
        .arg("--web-port")
        .arg(args.web_port.to_string())
        .arg("--addr")
        .arg(&args.addr)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    if let Some(web_addr) = &args.web_addr {
        command.arg("--web-addr").arg(web_addr);
    }
    if args.lan {
        command.arg("--lan");
    }
    let child = command.spawn().context("the daemon could not be spawned")?;
    std::fs::write(pid_file(), child.id().to_string())?;
    save_start_args(args)?;
    Ok(())
}

/// The price table in effect, and how to teach the gateway a new one.
async fn prices() -> anyhow::Result<()> {
    let table = pricing::effective_prices();
    println!(
        "local-router {} — {} models priced (per million tokens)",
        gateway::client::VERSION,
        table.len()
    );
    println!();
    println!(
        "{:<30} {:>10} {:>10} {:>12}",
        "MODEL", "IN $", "OUT $", "CREDITS/M"
    );
    for (id, price) in &table {
        println!(
            "{:<30} {:>10.4} {:>10.4} {:>12}",
            id,
            price.input,
            price.output,
            price
                .credits_per_million
                .map(|c| format!("{c:.1}"))
                .unwrap_or_else(|| "-".to_string())
        );
    }
    println!();
    if pricing::price_file_exists() {
        println!(
            "override file: {} (applied)",
            pricing::price_file().display()
        );
    } else {
        println!(
            "override file: {} (not present — write one to add or reprice models)",
            pricing::price_file().display()
        );
        println!("format: {{\"<model id>\": {{\"input\": 0.28, \"output\": 1.12, \"credits_per_million\": 120.0}}}}");
    }
    Ok(())
}

/// A timestamp, in a readable form.
fn format_time(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

// ---------------------------------------------------------------- groups --

async fn group_cmd(cmd: GroupCmd) -> anyhow::Result<()> {
    let mut config = load_config()?;
    match cmd {
        GroupCmd::Add {
            name,
            models,
            routing,
            stays,
        } => {
            let id = slugify(&name);
            let routing = match routing.as_str() {
                "smart" => Routing::Smart,
                "order" => Routing::Order,
                "rotate" => Routing::Rotate,
                "usage" => Routing::Usage,
                "weight" => Routing::Weight,
                other => anyhow::bail!("{other} is not a routing mode"),
            };
            let stays = match stays.as_str() {
                "auto" => Stays::Auto,
                "session" => Stays::Session,
                "turn" => Stays::Turn,
                "off" => Stays::Off,
                "strict" => Stays::Strict,
                other => anyhow::bail!("{other} is not a stays mode"),
            };
            if config.groups.iter().any(|g| g.id == id) {
                anyhow::bail!("there is already a group called {id}");
            }
            config.groups.push(config::Group {
                id: id.clone(),
                name,
                models,
                routing,
                weights: Default::default(),
                stays,
                listed: true,
                note: None,
            });
            save_config(&config)?;
            println!("added the group {id}");
        }
        GroupCmd::List => {
            if config.groups.is_empty() {
                println!("no groups yet");
                return Ok(());
            }
            for g in &config.groups {
                println!(
                    "{:<16} {:<8} {:<8} {}",
                    g.id,
                    g.routing.as_str(),
                    g.stays.as_str(),
                    g.models.join(", ")
                );
            }
        }
        GroupCmd::Rm { id } => {
            let before = config.groups.len();
            config.groups.retain(|g| g.id != id);
            if config.groups.len() == before {
                anyhow::bail!("there is no group called {id}");
            }
            save_config(&config)?;
            println!("removed the group {id}");
        }
    }
    Ok(())
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "group".to_string()
    } else {
        out
    }
}

// ----------------------------------------------------------------- views --

async fn models() -> anyhow::Result<()> {
    let config = load_config()?;
    for id in config.model_ids() {
        println!("{id}");
    }
    Ok(())
}

async fn ls() -> anyhow::Result<()> {
    let config = load_config()?;
    println!("providers:");
    if config.providers.is_empty() {
        println!("  (none yet — add one with: local-router provider add deepseek sk-…)");
    }
    for p in &config.providers {
        println!(
            "  {:<16} {:<6} {} model(s), {}",
            p.id,
            if p.enabled { "on" } else { "off" },
            p.models.len(),
            if p.uses_session() {
                "登录态".to_string()
            } else {
                format!("{} key(s)", p.keys.len())
            }
        );
    }
    println!();
    println!("groups:");
    if config.groups.is_empty() {
        println!("  (none yet)");
    }
    for g in &config.groups {
        println!(
            "  {:<16} {:<8} {}",
            g.id,
            g.routing.as_str(),
            g.models.join(", ")
        );
    }
    Ok(())
}

async fn usage(args: UsageArgs) -> anyhow::Result<()> {
    // SQLite answers with aggregates; the JSONL log is the fallback.
    let period_days: i64 = match args.period.as_str() {
        "today" => 1,
        "7d" => 7,
        "30d" => 30,
        _ => 3650,
    };
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(period_days)).to_rfc3339();
    let records = match store::Store::shared() {
        Some(store) => store.recent(&cutoff, usize::MAX),
        None => gateway::usage::read_all(&Store::new().usage_path())
            .into_iter()
            .filter(|r| r.time.as_str() >= cutoff.as_str())
            .collect(),
    };
    let recent = records;

    if args.csv {
        println!(
            "time,model,provider,input_tokens,output_tokens,cache_read,cache_write,latency_ms,ok,cost_usd,credits"
        );
        for r in &recent {
            println!(
                "{},{},{},{},{},{},{},{},{},{},{}",
                r.time,
                r.model,
                r.provider,
                r.input_tokens,
                r.output_tokens,
                r.cache_read_tokens,
                r.cache_write_tokens,
                r.latency_ms,
                r.ok,
                r.cost_usd.map(|c| format!("{c:.6}")).unwrap_or_default(),
                r.credits.map(|c| format!("{c:.4}")).unwrap_or_default(),
            );
        }
        return Ok(());
    }

    let totals = gateway::usage::totals(&recent);
    println!(
        "{} requests, {} failed, {} in, {} out",
        totals.requests, totals.failed, totals.input_tokens, totals.output_tokens
    );
    println!(
        "  cost      ${:.4}  ({unpriced} unpriced)",
        totals.cost_usd,
        unpriced = totals.unpriced
    );
    println!("  credits   {:.1}", totals.credits);
    println!();

    println!("by provider (every platform's own tab):");
    for (provider, t) in gateway::usage::group_by(&recent, |r| r.provider.clone()) {
        println!(
            "  {:<16} {:>5} req  {:>9} in  {:>8} out  ${:<8.4} {:>9.1} credits",
            provider, t.requests, t.input_tokens, t.output_tokens, t.cost_usd, t.credits
        );
    }
    println!();
    println!("by model:");
    for (model, t) in gateway::usage::group_by(&recent, |r| r.model.clone()) {
        println!(
            "  {:<32} {:>5} requests  {:>9} in  {:>8} out  ${:.4}",
            model, t.requests, t.input_tokens, t.output_tokens, t.cost_usd
        );
    }
    Ok(())
}

async fn set_agent(which: &str, args: AgentModelArgs) -> anyhow::Result<()> {
    let config = load_config()?;
    let settings = config.settings.clone();
    let gateway = local_router::agents::Gateway::new(
        &settings.gateway_addr,
        settings.gateway_port,
        &settings.default_caller_key,
    );

    let path = match which {
        "claude" => local_router::agents::set_claude(&gateway, Some(&args.model))?,
        "codex" => local_router::agents::set_codex(&gateway, Some(&args.model))?,
        "gemini" => local_router::agents::set_gemini(&gateway, Some(&args.model))?,
        "opencode" => local_router::agents::set_opencode(&gateway, Some(&args.model))?,
        other => anyhow::bail!("{other} is not an agent local-router knows"),
    };
    println!(
        "{which} now uses {} — written to {}",
        args.model,
        path.display()
    );
    println!("start a new session for it to take effect");
    Ok(())
}

/// Whether a provider is one that needs no key at all.
fn keyless(id: &str) -> bool {
    presets::get(id)
        .map(|preset| preset.key_hint.contains("(none needed)"))
        .unwrap_or(false)
}

async fn doctor() -> anyhow::Result<()> {
    let config = load_config()?;
    println!("local-router {}", gateway::client::VERSION);
    println!("  config      {}", config::config_file().display());
    println!(
        "  gateway     http://{}:{}",
        config.settings.gateway_addr, config.settings.gateway_port
    );
    println!(
        "  web         http://{}:{}",
        config.settings.web_addr, config.settings.web_port
    );
    println!("  providers   {}", config.providers.len());
    println!("  groups      {}", config.groups.len());
    println!("  models      {}", config.model_ids().len());

    let mut problems = Vec::new();
    for p in &config.providers {
        if p.uses_session() {
            // A WorkBuddy provider authenticates with the local app's
            // session, so the only thing to check is that there is one.
            match crate::workbuddy::read_session_file() {
                Ok(session) => {
                    let left = session
                        .expires_at
                        .map(|at| (at - crate::workbuddy::now_ms()) / 3_600_000)
                        .unwrap_or_default();
                    println!(
                        "  workbuddy   signed in as {} (token {} h left)",
                        if session.account.nickname.is_empty() {
                            "unknown"
                        } else {
                            &session.account.nickname
                        },
                        left
                    );
                }
                Err(err) => problems.push(format!(
                    "the provider {} needs a WorkBuddy login: {err}",
                    p.id
                )),
            }
        } else if p.enabled && p.enabled_keys().next().is_none() && !keyless(&p.id) {
            problems.push(format!("the provider {} has no key", p.id));
        }
        if p.enabled && p.base_url.is_none() {
            problems.push(format!("the provider {} has no url", p.id));
        }
    }
    for g in &config.groups {
        for m in &g.models {
            if let Some((pid, _)) = config::Config::split_model(m) {
                if config.provider(pid).is_none() {
                    problems.push(format!(
                        "the group {} names an unknown provider {pid}",
                        g.id
                    ));
                }
            }
        }
    }
    if problems.is_empty() {
        println!("  everything looks in order");
    } else {
        for p in problems {
            println!("  ! {p}");
        }
    }
    Ok(())
}
