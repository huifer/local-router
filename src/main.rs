//! local-router: a local gateway that gives every coding agent every model.
//!
//! The web page and the gateway are the same process. `local-router` starts
//! both; `local-router serve` starts the gateway alone; `local-router
//! console` drops the user into a small interactive prompt.

pub(crate) mod console;

use local_router::{
    config, gateway, presets, pricing, protocol, qwen, server, store, web, workbuddy,
};

use std::collections::BTreeMap;
use std::io::IsTerminal;
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
    /// The interactive console: one short word per line, no flags to remember.
    Console,
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
    /// QwenWork（千问办公）: the device-flow login, its credits and its models.
    #[command(name = "qwen", subcommand)]
    Qwen(QwenCmd),
    /// Show the price table the gateway bills with.
    Prices,
}

#[derive(Subcommand, Debug)]
enum DaemonCmd {
    /// Start the daemon in the background (gateway + console, one process).
    Start {
        /// The port the daemon listens on: the gateway and the console
        /// share it. Defaults to the file's setting, then the environment,
        /// then 3435.
        #[arg(long)]
        port: Option<u16>,
        /// Deprecated: the console is on the daemon's port now.
        #[arg(long, env = "LOCAL_ROUTER_WEB_PORT", default_value_t = 0, hide = true)]
        web_port: u16,
        /// The address to bind. Defaults to the file's setting, then the
        /// environment, then 127.0.0.1.
        #[arg(long)]
        addr: Option<String>,
        /// Deprecated: the console binds with the gateway.
        #[arg(long, env = "LOCAL_ROUTER_WEB_ADDR", hide = true)]
        web_addr: Option<String>,
        /// Bind on the local network (the address becomes 0.0.0.0).
        #[arg(long)]
        lan: bool,
        /// Wait until the ports answer, then return.
        #[arg(long, default_value_t = true)]
        wait: bool,
    },
    /// Stop the running daemon.
    Stop,
    /// Run the daemon in the foreground (what launchd / systemd run).
    Run,
    /// Start the daemon at login (macOS: launchd, Linux: systemd).
    Install,
    /// Remove the login item.
    Uninstall,
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
    Login {
        /// The name to file this account under (several accounts can live
        /// side by side). Empty picks the first free slot.
        #[arg(long, default_value = "")]
        account: String,
        /// Only print the link; do not try to open a browser.
        #[arg(long)]
        no_open: bool,
    },
    /// List the WorkBuddy accounts local-router holds.
    Accounts,
    /// Use another account from now on.
    Use { name: String },
}

#[derive(Subcommand, Debug)]
enum QwenCmd {
    /// Show whether QwenWork is signed in.
    Status,
    /// Show the credits on the account.
    Credits,
    /// List the models the account may use.
    Models,
    /// Sign in to QwenWork: print a link, wait for the browser, wire it up.
    Login {
        /// The name to file this account under (several accounts can live
        /// side by side). Empty picks the first free slot.
        #[arg(long, default_value = "")]
        account: String,
        /// Only print the link; do not try to open a browser.
        #[arg(long)]
        no_open: bool,
    },
    /// List the QwenWork accounts local-router holds.
    Accounts,
    /// Use another account from now on.
    Use { name: String },
}

#[derive(Args, Debug, Clone, Default)]
struct ServeArgs {
    /// The port local-router listens on: the gateway and the console share
    /// it. Zero lets the system pick a free one. Defaults to the file's
    /// setting, then LOCAL_ROUTER_GATEWAY_PORT, then 3435.
    #[arg(long)]
    port: Option<u16>,
    /// Deprecated: the console is on the gateway's port now. Zero still
    /// turns the console off.
    #[arg(long, env = "LOCAL_ROUTER_WEB_PORT", default_value_t = 0, hide = true)]
    web_port: u16,
    /// Serve the model API only, without the console.
    #[arg(long)]
    no_console: bool,
    /// Bind on the local network, so other computers can use the gateway.
    #[arg(long)]
    lan: bool,
    /// The address to bind. Defaults to the file's setting, then
    /// LOCAL_ROUTER_GATEWAY_ADDR, then 127.0.0.1.
    #[arg(long)]
    addr: Option<String>,
    /// Deprecated: the console binds with the gateway.
    #[arg(long, env = "LOCAL_ROUTER_WEB_ADDR", hide = true)]
    web_addr: Option<String>,
}

#[derive(Args, Debug, Clone)]
struct WebArgs {
    /// The port the console listens on.
    #[arg(long, env = "LOCAL_ROUTER_WEB_PORT", default_value_t = 3426)]
    port: u16,
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
    /// Show a provider's header rules.
    Headers { id: String },
    /// Set the static headers a provider sends (name=value, repeated).
    SetHeaders {
        id: String,
        /// name=value pairs; an empty list clears them all.
        #[arg(value_delimiter = ' ')]
        rules: Vec<String>,
    },
    /// Set which caller headers are forwarded to this provider.
    ForwardHeaders {
        id: String,
        /// Header names, comma separated; empty clears the list.
        #[arg(default_value = "", value_delimiter = ',')]
        names: Vec<String>,
    },
    /// Set which headers are dropped before the request goes out.
    RemoveHeaders {
        id: String,
        /// Header names, comma separated; empty clears the list.
        #[arg(default_value = "", value_delimiter = ',')]
        names: Vec<String>,
    },
    /// Set the order the upstream headers are sent in.
    HeaderOrder {
        id: String,
        /// Header names, comma separated; empty clears the hint.
        #[arg(default_value = "", value_delimiter = ',')]
        names: Vec<String>,
    },
    /// Set the User-Agent template ({version} / {platform} / {arch} expand).
    UserAgent {
        id: String,
        /// The template, or empty to stop sending one.
        #[arg(default_value = "")]
        template: String,
    },
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
    /// Set the weights of a weight-routed group (member=weight, comma separated).
    Weights {
        id: String,
        /// e.g. `deepseek/deepseek-chat=3,glm/glm-4.6=1`; empty clears them.
        #[arg(default_value = "", value_delimiter = ',')]
        rules: Vec<String>,
    },
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
        Some(Command::Console) => console::run().await,
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
        Some(Command::Qwen(cmd)) => qwen_cmd(cmd).await,
        Some(Command::Prices) => prices().await,
        Some(Command::Dashboard) => local_router::dashboard::run().await,
        Some(Command::Daemon(cmd)) => daemon_cmd(cmd).await,
        // Bare `local-router`: a terminal wants the gateway; a pipe (a
        // beginner pasting into the wrong window, a script) wants the
        // friendly console. Tell the two apart by whether stdin is a tty.
        None => {
            if std::io::stdin().is_terminal() {
                serve(ServeArgs::default()).await
            } else {
                println!(
                    "(看起来不是交互终端；进入交互模式。要直接启动网关，运行 local-router serve)"
                );
                console::run().await
            }
        }
    }
}

// ---------------------------------------------------------------- servers --

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let store = Arc::new(Store::new());
    let state = AppState::load(store).await?;
    let config = state.config.read().await;
    // Environment first (containers), then the command line (people).
    // The file first, then the environment, then the command line: the
    // command line wins only when it was actually given.
    let mut settings = config.settings.clone().from_env();
    if let Some(port) = args.port {
        settings.gateway_port = port;
    }
    if let Some(addr) = &args.addr {
        settings.gateway_addr = addr.trim().to_string();
    }
    if args.lan || settings.lan {
        settings.gateway_addr = "0.0.0.0".to_string();
    }
    // The console lives on the gateway's port now. The switch that decides
    // whether it is served at all is `--no-console` (or `console: false` in
    // the file); a legacy `web_port` is only a leftover.
    let console = !args.no_console && settings.console;
    if args.web_port != 0 && Some(args.web_port) != args.port {
        eprintln!(
            "note: the console now shares the gateway's port; --web-port {} is ignored",
            args.web_port
        );
    }
    drop(config);
    // Handlers that care — the console's remote guard — must see the real
    // bind address, not the file's. The console is on the same port now, so
    // it is exposed exactly when the gateway is.
    state.set_web_bind(&settings.gateway_addr).await;

    if settings.workbuddy_auto_checkin {
        start_auto_checkin();
    }

    // `--port 0` lets the system pick a free port: handy when another
    // gateway (magpie, an older local-router) already holds 3435.
    let addr = format!("{}:{}", settings.gateway_addr, settings.gateway_port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|err| port_taken_error(&addr, err))?;
    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let addr = format!("{}:{}", settings.gateway_addr, actual_port);

    println!("local-router {}", gateway::client::VERSION);
    println!("  gateway  http://{addr}");
    if console {
        println!("  console  http://{addr}");
    }
    println!("  press ctrl-c to stop");

    let router = if console {
        server::router(state.clone())
    } else {
        gateway::router(gateway::Gateway::new(state.clone()))
    };

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown())
        .await
        .context("local-router stopped")?;
    Ok(())
}

/// Serve the console alone — for the case where the gateway runs
/// somewhere else and this machine only wants the page.
async fn web_only(args: WebArgs) -> anyhow::Result<()> {
    let store = Arc::new(Store::new());
    let state = AppState::load(store).await?;
    let settings = state.config.read().await.settings.clone().from_env();
    state.set_web_bind(&settings.gateway_addr).await;
    if settings.workbuddy_auto_checkin {
        start_auto_checkin();
    }

    let web_addr = format!("{}:{}", settings.gateway_addr, args.port);
    let listener = tokio::net::TcpListener::bind(&web_addr)
        .await
        .map_err(|err| port_taken_error(&web_addr, err))?;
    println!("  console  http://{web_addr}");
    println!("  press ctrl-c to stop");
    axum::serve(listener, web::router(state))
        .with_graceful_shutdown(shutdown())
        .await
        .context("the console stopped")?;
    Ok(())
}

/// The error to show when the port is already taken.
///
/// A taken port is the most common way to fail to start, and the raw
/// "Address already in use" says nothing about what to do next. Name the
/// port, name the usual suspects, and hand over a command that works.
fn port_taken_error(addr: &str, err: std::io::Error) -> anyhow::Error {
    let taken = err.kind() == std::io::ErrorKind::AddrInUse;
    if !taken {
        return anyhow::Error::new(err).context(format!("local-router could not listen on {addr}"));
    }
    let port = addr.rsplit(':').next().unwrap_or(addr);
    anyhow::anyhow!(
        "the port {port} is already taken — another gateway (magpie? an older local-router?) is \
         using it.\n\n  Start local-router on another port:\n    local-router serve --port {}\n\n           Or let the system pick a free one:\n    local-router serve --port 0\n\n           To see who holds it: lsof -nP -iTCP:{port} -sTCP:LISTEN",
        port.parse::<u16>().unwrap_or(0) + 10
    )
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
            for m in &models {
                println!("  {m}");
            }
            // The provider's auto_include globs decide which of them the
            // gateway exposes; the new ones are folded in and saved, so a
            // vendor that renames its models needs no hand-editing.
            let patterns = provider.auto_include.clone();
            if !patterns.is_empty() {
                let mut added = Vec::new();
                for m in &models {
                    let known = config
                        .provider(&id)
                        .map(|p| p.models.contains(m))
                        .unwrap_or(false);
                    if !known && patterns.iter().any(|pat| glob_match(pat, m)) {
                        added.push(m.clone());
                    }
                }
                if !added.is_empty() {
                    let count = added.len();
                    {
                        let Some(p) = config.provider_mut(&id) else {
                            anyhow::bail!("there is no provider called {id}");
                        };
                        p.models.extend(added.iter().cloned());
                        p.models.sort();
                        p.models.dedup();
                    }
                    save_config(&config)?;
                    println!("  + {count} model(s) exposed by auto_include");
                }
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
        ProviderCmd::Headers { id } => {
            let Some(p) = config.provider(&id) else {
                anyhow::bail!("there is no provider called {id}");
            };
            println!("{id} 的请求头规则：");
            if p.headers.is_empty() {
                println!("  静态头    (none)");
            } else {
                println!("  静态头");
                for (k, v) in &p.headers {
                    println!("    {k}: {v}");
                }
            }
            println!(
                "  转发调用方  {}",
                if p.forward_headers.is_empty() {
                    "(none)".into()
                } else {
                    p.forward_headers.join(", ")
                }
            );
            println!(
                "  删除      {}",
                if p.remove_headers.is_empty() {
                    "(none)".into()
                } else {
                    p.remove_headers.join(", ")
                }
            );
            println!(
                "  发送顺序  {}",
                if p.header_order.is_empty() {
                    "(natural)".into()
                } else {
                    p.header_order.join(", ")
                }
            );
            println!(
                "  UA 模板    {}",
                p.user_agent.as_deref().unwrap_or("(local-router 默认)")
            );
        }
        ProviderCmd::SetHeaders { id, rules } => {
            let mut headers = std::collections::BTreeMap::new();
            for rule in &rules {
                let Some((name, value)) = rule.split_once('=') else {
                    anyhow::bail!("{rule} 不是 name=value 形式");
                };
                headers.insert(name.trim().to_string(), value.trim().to_string());
            }
            let count = headers.len();
            {
                let Some(p) = config.provider_mut(&id) else {
                    anyhow::bail!("there is no provider called {id}");
                };
                p.headers = headers;
            }
            save_config(&config)?;
            println!("{id} 现在发送 {count} 个静态头");
        }
        ProviderCmd::ForwardHeaders { id, names } => {
            let forwarded: Vec<String> = names
                .iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            {
                let Some(p) = config.provider_mut(&id) else {
                    anyhow::bail!("there is no provider called {id}");
                };
                p.forward_headers = forwarded.clone();
            }
            save_config(&config)?;
            println!(
                "{id} 转发调用方的 {}",
                if forwarded.is_empty() {
                    "(none)".into()
                } else {
                    forwarded.join(", ")
                }
            );
        }
        ProviderCmd::RemoveHeaders { id, names } => {
            let removed: Vec<String> = names
                .iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            {
                let Some(p) = config.provider_mut(&id) else {
                    anyhow::bail!("there is no provider called {id}");
                };
                p.remove_headers = removed.clone();
            }
            save_config(&config)?;
            println!(
                "{id} 删除 {}",
                if removed.is_empty() {
                    "(none)".into()
                } else {
                    removed.join(", ")
                }
            );
        }
        ProviderCmd::HeaderOrder { id, names } => {
            let order: Vec<String> = names
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            {
                let Some(p) = config.provider_mut(&id) else {
                    anyhow::bail!("there is no provider called {id}");
                };
                p.header_order = order.clone();
            }
            save_config(&config)?;
            println!(
                "{id} 发送顺序 {}",
                if order.is_empty() {
                    "(natural)".into()
                } else {
                    order.join(", ")
                }
            );
        }
        ProviderCmd::UserAgent { id, template } => {
            let ua = (!template.trim().is_empty()).then(|| template.trim().to_string());
            {
                let Some(p) = config.provider_mut(&id) else {
                    anyhow::bail!("there is no provider called {id}");
                };
                p.user_agent = ua.clone();
            }
            save_config(&config)?;
            match &ua {
                Some(t) => {
                    println!("{id} 的 UA 模板：{t}（{{version}}/{{platform}}/{{arch}} 会展开）")
                }
                None => println!("{id} 不再发送自定义 UA"),
            }
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
            gateway::client::auth_headers(provider.protocol, provider, &key.key, None, None),
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
        WorkBuddyCmd::Login { account, no_open } => login(&account, no_open).await?,
        WorkBuddyCmd::Accounts => {
            let accounts = workbuddy::accounts()?;
            if accounts.is_empty() {
                println!("no WorkBuddy accounts yet; run: local-router workbuddy login");
            } else {
                println!("{:<16} {:<20} TOKEN", "ACCOUNT", "NICKNAME");
                for (name, file) in accounts {
                    let left = file
                        .auth
                        .expires_at
                        .map(|at| format!("until {}", format_time(at)))
                        .unwrap_or_else(|| "unknown".to_string());
                    println!(
                        "{:<16} {:<20} {left}",
                        name,
                        if file.account.nickname.is_empty() {
                            file.account.uid.as_str()
                        } else {
                            file.account.nickname.as_str()
                        }
                    );
                }
                let pool = workbuddy::read_pool()?;
                println!();
                println!(
                    "active: {}",
                    if pool.active.is_empty() {
                        "(none)"
                    } else {
                        &pool.active
                    }
                );
            }
        }
        WorkBuddyCmd::Use { name } => {
            workbuddy::use_account(&name)?;
            println!("now using the WorkBuddy account {name}");
        }
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
async fn login(account_name: &str, no_open: bool) -> anyhow::Result<()> {
    // An empty name takes the first free slot, so a second sign-in never
    // overwrites the first account.
    let taken: Vec<String> = workbuddy::read_pool()
        .map(|p| p.accounts.keys().cloned().collect())
        .unwrap_or_default();
    let name = if account_name.trim().is_empty() {
        first_free_slot("workbuddy", &taken)
    } else {
        account_name.trim().to_string()
    };
    let session = workbuddy::start_login().await?;
    println!("local-router {}", gateway::client::VERSION);
    println!("  open this link in a browser and sign in:");
    println!();
    println!("  {}", session.auth_url);
    println!();

    // Open the default browser for the person at the keyboard. When it
    // cannot (a headless box, a remote shell), the link above is all they
    // need — it is always printed.
    if no_open {
        println!("  (not opening a browser; open the link above yourself)");
    } else {
        let _ = std::process::Command::new("open")
            .arg(&session.auth_url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

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
    let stored = workbuddy::store_session(&result.session, &account, &name).await?;
    println!("  signed in   {nickname}（账号名 {}）", stored.slot);
    if stored.same_uid_elsewhere {
        println!("  note        池里已有同一个 uid 的账号（浏览器可能自动登录了旧账号）");
    }
    println!("  session     {}", stored.path.display());
    println!(
        "  token until {}",
        result
            .session
            .expires_at
            .map(format_time)
            .unwrap_or_else(|| "unknown".to_string())
    );

    // Signing in is only the first step. Wire the provider, pull the
    // vendor's model list and claim the day's check-in here, so the user
    // does not have to run four more commands.
    let mut config = load_config()?;
    if config.provider("workbuddy").is_none() {
        if let Some(preset) = presets::get("workbuddy") {
            config.providers.push(presets::to_provider(preset, None));
            save_config(&config)?;
            println!("  provider    workbuddy added (no key needed)");
        }
    }

    match workbuddy::models().await {
        Ok(list) => {
            let mut ids: Vec<String> = list.into_iter().collect();
            ids.sort();
            ids.dedup();
            let count = ids.len();
            let mut config = load_config()?;
            if let Some(provider) = config.provider_mut("workbuddy") {
                for id in ids {
                    if !provider.models.contains(&id) {
                        provider.models.push(id);
                    }
                }
                provider.models.sort();
                provider.models.dedup();
                save_config(&config)?;
                println!("  models      {count} imported from the vendor");
            }
        }
        Err(err) => println!("  models      could not be pulled: {err}"),
    }

    match workbuddy::claim_checkin().await {
        Ok(outcome) if outcome.granted => println!(
            "  check-in    claimed (+{} credits)",
            outcome.credit.unwrap_or(0)
        ),
        Ok(outcome) => println!("  check-in    {}", outcome.message),
        Err(err) => println!("  check-in    could not be claimed: {err}"),
    }

    println!();
    println!("  done — try it: local-router claude workbuddy/hy3");
    Ok(())
}

// ------------------------------------------------------------- qwenwork --

async fn qwen_cmd(cmd: QwenCmd) -> anyhow::Result<()> {
    match cmd {
        QwenCmd::Status => {
            let path = qwen::login::session_file();
            println!("local-router {}", gateway::client::VERSION);
            match qwen::login::read_session_file() {
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
                            let left = (at - qwen::login::now_ms()) / 1000;
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
        QwenCmd::Credits => {
            let session = qwen::login::session().await?;
            let quota = qwen::client::account_quota(&session).await?;
            println!("plan: {}", quota.plan);
            println!("balance: {}", quota.balance);
            if let Some(at) = quota.resets_at {
                println!("resets:  {at}");
            }
            for p in &quota.pools {
                println!("  {:<14} {}", p.name, p.balance);
            }
        }
        QwenCmd::Models => {
            let session = qwen::login::session().await?;
            let models = qwen::client::models(&session).await?;
            println!("the account may use {} models:", models.len());
            for m in models {
                println!("  {m}");
            }
        }
        QwenCmd::Login { account, no_open } => qwen_login(&account, no_open).await?,
        QwenCmd::Accounts => {
            // No file yet is the same as "no accounts": say so instead of
            // failing with an unreadable-file error.
            let pool = qwen::login::read_pool().unwrap_or_default();
            if pool.accounts.is_empty() {
                println!("no QwenWork accounts yet; run: local-router qwen login");
                return Ok(());
            }
            println!("{} QwenWork account(s):", pool.accounts.len());
            for (name, file) in &pool.accounts {
                let marker = if *name == pool.active { "*" } else { " " };
                let who = if file.account.nickname.is_empty() {
                    file.account.email.clone()
                } else {
                    file.account.nickname.clone()
                };
                println!("  {marker} {name:<12} {who}");
            }
            println!("  (* = the account in use)");
        }
        QwenCmd::Use { name } => {
            qwen::login::use_account(&name)?;
            println!("now using the QwenWork account {name}");
        }
    }
    Ok(())
}

/// Sign in to QwenWork from the terminal.
///
/// The device flow hands out a browser link; the browser signs in; the
/// gateway polls until the device token arrives. The token is stored in
/// local-router's own session file — QwenWork's own data is left untouched.
///
/// Several accounts can live side by side: the one just signed in is filed
/// under `account` and becomes the account in use.
async fn qwen_login(account_name: &str, no_open: bool) -> anyhow::Result<()> {
    // An empty name takes the first free slot, so a second sign-in never
    // overwrites the first account.
    let taken: Vec<String> = qwen::login::read_pool()
        .map(|p| p.accounts.keys().cloned().collect())
        .unwrap_or_default();
    let name = if account_name.trim().is_empty() {
        first_free_slot("qwenwork", &taken)
    } else {
        account_name.trim().to_string()
    };
    let session = qwen::login::start_login().await?;
    println!("local-router {}", gateway::client::VERSION);
    println!("  open this link in a browser and sign in:");
    println!();
    println!("  {}", session.auth_url);
    println!();

    // The link is always printed; opening a browser is a convenience.
    if no_open {
        println!("  (not opening a browser; open the link above yourself)");
    } else {
        let _ = std::process::Command::new("open")
            .arg(&session.auth_url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    println!("  waiting for the sign-in (up to 5 minutes)…");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5 * 60);
    let token = loop {
        if std::time::Instant::now() > deadline {
            anyhow::bail!("the sign-in timed out; run `local-router qwen login` again");
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        match qwen::login::poll_login(&session.state).await {
            Ok(qwen::login::LoginPoll::Waiting) => continue,
            Ok(qwen::login::LoginPoll::Ready(result)) => break *result,
            Err(err) => return Err(err),
        }
    };

    // Who signed in, from the adapter account-context.
    let account = qwen::login::login_account(&session.state, &token)
        .await
        .unwrap_or(serde_json::Value::Null);
    let nickname = account
        .pointer("/data/user/name")
        .or_else(|| account.pointer("/user/name"))
        .or_else(|| account.pointer("/name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");

    let stored =
        qwen::login::store_session(&token, &account, &session.state.machine_id, &name).await?;
    println!("  signed in   {nickname}");
    println!("  account     {}", stored.slot);
    if stored.same_uid_elsewhere {
        println!("  note        池里已有同一个 uid 的账号（浏览器可能自动登录了旧账号）");
    }
    println!("  session     {}", stored.path.display());
    println!(
        "  token until {}",
        token
            .expires_at
            .map(format_time)
            .unwrap_or_else(|| "unknown".to_string())
    );

    let mut config = load_config()?;
    if config.provider("qwenwork").is_none() {
        if let Some(preset) = presets::get("qwenwork") {
            config.providers.push(presets::to_provider(preset, None));
            save_config(&config)?;
            println!("  provider    qwenwork added (no key needed)");
        }
    }

    println!();
    println!("  done — try it: local-router qwen credits");
    Ok(())
}

// ---------------------------------------------------------------- daemon --

/// Where the daemon's bookkeeping lives.
/// The login item that starts the daemon with the machine.
#[cfg(target_os = "macos")]
fn install_login_item() -> anyhow::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    let plist_dir = dirs::home_dir()
        .context("no home directory")?
        .join("Library/LaunchAgents");
    std::fs::create_dir_all(&plist_dir)?;
    let path = plist_dir.join("com.local-router.daemon.plist");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.local-router.daemon</string>
  <key>ProgramArguments</key>
  <array><string>{}</string><string>daemon</string><string>run</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{}</string>
  <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
        exe.display(),
        log_file().display(),
        log_file().display(),
    );
    std::fs::write(&path, plist)?;
    // Ask launchd to load it now, so the daemon is up without a logout.
    let _ = std::process::Command::new("launchctl")
        .args(["load", "-w", &path.to_string_lossy()])
        .status();
    Ok(path)
}

#[cfg(target_os = "macos")]
fn remove_login_item() -> anyhow::Result<std::path::PathBuf> {
    let path = dirs::home_dir()
        .context("no home directory")?
        .join("Library/LaunchAgents/com.local-router.daemon.plist");
    if path.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["unload", "-w", &path.to_string_lossy()])
            .status();
        std::fs::remove_file(&path)?;
    }
    Ok(path)
}

#[cfg(not(target_os = "macos"))]
fn install_login_item() -> anyhow::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = dirs::home_dir()
        .context("no home directory")?
        .join(".config/systemd/user");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("local-router.service");
    let unit = format!(
        "[Unit]
Description=local-router gateway

[Service]
ExecStart={} daemon start
Restart=always

[Install]
WantedBy=default.target
",
        exe.display()
    );
    std::fs::write(&path, unit)?;
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "local-router.service"])
        .status();
    Ok(path)
}

#[cfg(not(target_os = "macos"))]
fn remove_login_item() -> anyhow::Result<std::path::PathBuf> {
    let path = dirs::home_dir()
        .context("no home directory")?
        .join(".config/systemd/user/local-router.service");
    if path.exists() {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "disable", "--now", "local-router.service"])
            .status();
        std::fs::remove_file(&path)?;
    }
    Ok(path)
}

fn daemon_dir() -> std::path::PathBuf {
    config::config_dir().join("daemon")
}

fn pid_file() -> std::path::PathBuf {
    daemon_dir().join("daemon.pid")
}

fn log_file() -> std::path::PathBuf {
    daemon_dir().join("daemon.log")
}

/// Whether a process is still alive.
fn process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
        || std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
}

/// The pid of the running daemon, if there is one and it is alive.
fn running_pid() -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(pid_file())
        .ok()?
        .trim()
        .parse()
        .ok()?;
    // A pid file without a live process is a leftover: clean it up.
    if process_alive(pid) {
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
            // The effective address: the file first, then the environment,
            // then the command line (which is what the child will use).
            let (port, addr) = effective_address(&args);
            let child_pid = start_daemon(&args)?;
            if wait {
                // Wait until the port answers, or say what went wrong.
                let gateway = format!("http://{addr}:{port}/health");
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                loop {
                    let gateway_up = reqwest::get(&gateway)
                        .await
                        .map(|r| r.status().is_success())
                        .unwrap_or(false);
                    if gateway_up {
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
            // The child may have died right away — the port was taken, for
            // example. Saying "started" then would be a lie.
            if !process_alive(child_pid) {
                anyhow::bail!(
                    "the daemon did not stay up (the port may be in use); see {}",
                    log_file().display()
                );
            }
            println!("daemon started  gateway + console http://{}:{}", addr, port);
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
        DaemonCmd::Run => {
            // Foreground, with the pid file written so `daemon status` still
            // works: launchd and systemd want a process that stays.
            let _ = std::fs::create_dir_all(daemon_dir());
            let _ = std::fs::write(pid_file(), std::process::id().to_string());
            let result = serve(ServeArgs::default()).await;
            // Only clear the pid file when it is still ours: a daemon that
            // starts later may have taken the file over already, and
            // deleting it then would leave that one unreachable.
            if running_pid() == Some(std::process::id()) {
                let _ = std::fs::remove_file(pid_file());
            }
            result
        }
        DaemonCmd::Install => {
            let path = install_login_item()?;
            println!("installed: {}", path.display());
            println!("the gateway now starts with your machine");
            Ok(())
        }
        DaemonCmd::Uninstall => {
            let path = remove_login_item()?;
            println!("removed: {}", path.display());
            Ok(())
        }
        DaemonCmd::Status => {
            match running_pid() {
                Some(pid) => {
                    let args = read_start_args().unwrap_or_default();
                    let port = args.port.unwrap_or(3435);
                    let addr = args.addr.clone().unwrap_or_else(|| "127.0.0.1".to_string());
                    println!("running    yes (pid {pid})");
                    println!("gateway    http://{addr}:{port}");
                    println!("console    http://{addr}:{port}  (same port)");
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
    port: Option<u16>,
    web_port: u16,
    addr: Option<String>,
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
/// The port and address the daemon will actually bind: the file's setting,
/// then the environment, then the command line.
fn effective_address(args: &DaemonStart) -> (u16, String) {
    let settings = load_config()
        .map(|c| c.settings)
        .unwrap_or_default()
        .from_env();
    let port = args.port.unwrap_or(settings.gateway_port);
    let addr = args
        .addr
        .clone()
        .unwrap_or(settings.gateway_addr)
        .trim()
        .to_string();
    (port, addr)
}

fn start_daemon(args: &DaemonStart) -> anyhow::Result<u32> {
    std::fs::create_dir_all(daemon_dir())?;
    let exe = std::env::current_exe()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file())?;

    let mut command = std::process::Command::new(exe);
    command
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // Only pass what was actually given: the daemon inherits the file's
    // settings otherwise.
    if let Some(port) = args.port {
        command.arg("--port").arg(port.to_string());
    }
    if let Some(addr) = &args.addr {
        command.arg("--addr").arg(addr);
    }
    if args.lan {
        command.arg("--lan");
    }
    let child = command.spawn().context("the daemon could not be spawned")?;
    // Another daemon may already own the port — launchd's `daemon run`
    // does, and it wrote its own pid. Do not take the pid file away from
    // a live daemon: the new child would only fail on the port anyway.
    if running_pid().is_none() {
        std::fs::write(pid_file(), child.id().to_string())?;
    }
    save_start_args(args)?;
    Ok(child.id())
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

/// The first free slot name for a new account: `default`, then
/// `{provider}-2`, `{provider}-3`, … so a second sign-in never overwrites
/// the first account.
fn first_free_slot(provider_id: &str, taken: &[String]) -> String {
    if !taken.iter().any(|n| n == "default") {
        return "default".to_string();
    }
    let mut index = 2;
    loop {
        let candidate = format!("{provider_id}-{index}");
        if !taken.iter().any(|n| n == &candidate) {
            return candidate;
        }
        index += 1;
    }
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
        GroupCmd::Weights { id, rules } => {
            let mut weights: BTreeMap<String, u32> = BTreeMap::new();
            for rule in &rules {
                let Some((member, weight)) = rule.split_once('=') else {
                    anyhow::bail!("{rule} 不是 member=weight 形式");
                };
                let weight: u32 = weight
                    .trim()
                    .parse()
                    .with_context(|| format!("{weight} 不是一个数字（权重是 1–1000 的整数）"))?;
                weights.insert(member.trim().to_string(), weight.clamp(1, 1000));
            }
            {
                let Some(g) = config.group_mut(&id) else {
                    anyhow::bail!("there is no group called {id}");
                };
                g.weights = weights.clone();
            }
            save_config(&config)?;
            println!(
                "{id} 的权重：{}",
                if weights.is_empty() {
                    "(none)".to_string()
                } else {
                    weights
                        .iter()
                        .map(|(m, w)| format!("{m}={w}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            );
        }
    }
    Ok(())
}

/// Whether a model id matches one glob (`*` is the only wildcard).
fn glob_match(pattern: &str, model: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == model;
    }
    let mut split = pattern.split('*');
    let head = split.next().unwrap_or_default();
    let last = split.next_back().unwrap_or_default();
    model.len() >= head.len() + last.len() && model.starts_with(head) && model.ends_with(last)
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
    // The console is on the gateway's port now — one address, not two.
    println!(
        "  gateway     http://{}:{}",
        config.settings.gateway_addr, config.settings.gateway_port
    );
    if config.settings.console {
        println!(
            "  console     http://{}:{}  (same port)",
            config.settings.gateway_addr, config.settings.gateway_port
        );
    } else {
        println!("  console     off (settings.console = false)");
    }
    println!("  providers   {}", config.providers.len());
    println!("  groups      {}", config.groups.len());
    println!("  models      {}", config.model_ids().len());

    let mut problems = Vec::new();
    for p in &config.providers {
        if p.uses_session() {
            // A session provider authenticates with a local login, so the
            // only thing to check is that there is one.
            if p.id == "qwenwork" {
                match crate::qwen::login::read_session_file() {
                    Ok(session) => {
                        let left = session
                            .expires_at
                            .map(|at| (at - crate::qwen::login::now_ms()) / 3_600_000)
                            .unwrap_or_default();
                        println!(
                            "  qwenwork    signed in as {} (token {} h left)",
                            if session.account.nickname.is_empty() {
                                "unknown"
                            } else {
                                &session.account.nickname
                            },
                            left
                        );
                    }
                    Err(err) => problems.push(format!(
                        "the provider {} needs a QwenWork login: {err}",
                        p.id
                    )),
                }
            } else {
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
