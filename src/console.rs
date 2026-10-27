//! The interactive console: a friendly chat with local-router.
//!
//! Not everyone wants to type full subcommands. `local-router console`
//! (or just running local-router when stdin is not a terminal) drops the
//! user into a small read-eval-print loop — the way Python's `python`
//! works, but for the gateway: one short word per line, plain answers,
//! no flags to remember.
//!
//! It reuses the same code paths the subcommands use, so anything done
//! here is exactly what the CLI would do.

use std::io::Write;

use anyhow::Context;

use crate::config::{Config, Store};
use crate::{gateway, presets};

/// Commands the console understands. One keyword, mostly one argument.
pub const HELP: &str = "\
命令（敲一个词就行，不用记子命令）：

  help                 这份说明
  status               网关健康 + 有哪些 provider / group
  start | stop         启动 / 停止后台网关（daemon）
  add <厂商> [key]     加上游：add deepseek sk-xxx
  add <名> --url …     自定义上游：add 七牛云 --url https://api.qnaigc.com/v1 --key …
                       --protocol openai_chat（默认）/ anthropic / gemini / openai_responses
                       --models m1,m2（可省，稍后 fetch 或 expose）
  rm <厂商>            删上游
  list                 列出已配的上游
  models               网关对外暴露的模型
  fetch <厂商>         拉厂商自己的模型列表
  test <厂商>          测一个上游通不通
  expose <厂商> m1,m2  设置对外暴露哪些模型
  group <名> m1,m2     建一个路由组（主备，order）
  groups               列出路由组
  use <agent> <模型>   把 Claude Code / Codex / Gemini / OpenCode 指到某个模型
  usage                用量（近 7 天）
  doctor               自检
  prices               价目表
  workbuddy [子命令]    WorkBuddy：login 登录（--account 名字，可多账号；
                       --no-open 只打印链接不开浏览器）/
                       status 状态 / credits 积分 / models 模型 /
                       checkin 签到 / accounts 列出账号 / use <名字> 切换
  qwen [子命令]         QwenWork（千问办公）：login（链接会打印在终端里，
                       自己打开即可；--no-open 不自动开浏览器）/ status / credits / models
  open                 在浏览器打开控制台
  clear | clean        清屏
  exit | quit          退出（Ctrl-D 也行）
";

/// Run the interactive console. Returns when the user leaves.
pub async fn run() -> anyhow::Result<()> {
    println!("local-router 交互模式 — 敲 help 看命令，exit 退出。");
    loop {
        print!("local-router> ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => break, // EOF (Ctrl-D)
            Ok(_) => {}
            Err(err) => return Err(err).context("could not read the console input"),
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // `exit` / `quit` — the one way out that is not an error.
        if matches!(line, "exit" | "quit" | "q") {
            println!("bye");
            break;
        }
        match handle(line).await {
            Ok(()) => {}
            Err(err) => println!("! {err}"),
        }
    }
    Ok(())
}

/// One console line: split into words and dispatch. The pieces reuse the
/// CLI's own handlers where they are reusable, so behavior never drifts.
async fn handle(line: &str) -> anyhow::Result<()> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let cmd = words[0].to_ascii_lowercase();
    let rest = &words[1..];
    match cmd.as_str() {
        "help" | "h" | "?" => {
            println!("{HELP}");
        }
        "status" => status().await?,
        "start" => {
            super::daemon_cmd(super::DaemonCmd::Start {
                port: None,
                web_port: 0,
                addr: None,
                web_addr: None,
                lan: false,
                wait: true,
            })
            .await?
        }
        "stop" => super::daemon_cmd(super::DaemonCmd::Stop).await?,
        "add" => add(rest).await?,
        "rm" => {
            let Some(id) = rest.first() else {
                anyhow::bail!("要删哪个？rm deepseek");
            };
            super::provider_cmd(crate::ProviderCmd::Rm { id: id.to_string() }).await?;
        }
        "list" => super::provider_cmd(crate::ProviderCmd::List).await?,
        "models" => super::models().await?,
        "fetch" => {
            let Some(id) = rest.first() else {
                anyhow::bail!("要拉哪家的模型列表？fetch deepseek");
            };
            super::provider_cmd(crate::ProviderCmd::Models { id: id.to_string() }).await?;
        }
        "test" => {
            let Some(id) = rest.first() else {
                anyhow::bail!("要测哪个？test deepseek");
            };
            super::provider_cmd(crate::ProviderCmd::Test { id: id.to_string() }).await?;
        }
        "expose" => {
            let Some(id) = rest.first() else {
                anyhow::bail!("用法：expose deepseek deepseek-chat,deepseek-reasoner");
            };
            let models: Vec<String> = rest
                .get(1..)
                .unwrap_or(&[])
                .iter()
                .flat_map(|s| s.split(','))
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string())
                .collect();
            if models.is_empty() {
                anyhow::bail!("要暴露哪些模型？expose deepseek deepseek-chat");
            }
            super::provider_cmd(crate::ProviderCmd::Expose {
                id: id.to_string(),
                models,
            })
            .await?;
        }
        "group" => group(rest).await?,
        "groups" => super::group_cmd(crate::GroupCmd::List).await?,
        "use" => use_agent(rest).await?,
        "usage" => {
            super::usage(crate::UsageArgs {
                csv: false,
                period: "7d".into(),
            })
            .await?
        }
        "doctor" => super::doctor().await?,
        "prices" => super::prices().await?,
        "workbuddy" => workbuddy(rest).await?,
        "qwen" => qwen(rest).await?,
        "open" => open_console().await?,
        "clear" | "clean" | "cls" => {
            // ANSI: clear the screen and move home; a fallback for the
            // few terminals that ignore it.
            print!("\x1b[2J\x1b[H");
            let _ = std::io::stdout().flush();
        }
        "dashboard" => {
            return Err(anyhow::anyhow!(
                "dashboard 需要全屏终端；退出后运行 local-router dashboard"
            ))
        }
        "serve" => {
            return Err(anyhow::anyhow!(
                "前台网关会占住这个会话；先 exit，再运行 local-router serve"
            ))
        }
        other => {
            anyhow::bail!("不认识「{other}」。敲 help 看全部命令。")
        }
    }
    Ok(())
}

/// 网关健康 + 配置概览。控制台页面与 API 共用一个端口。
async fn status() -> anyhow::Result<()> {
    let config = load()?;
    println!("local-router {}", gateway::client::VERSION);
    let s = &config.settings;
    println!(
        "  地址      http://{}:{}（网关与控制台同一个端口）",
        s.gateway_addr, s.gateway_port
    );
    println!(
        "  后台网关  {}",
        super::running_pid().map_or("未运行（start 启动）".into(), |pid| format!(
            "运行中 (pid {pid})"
        ))
    );
    println!(
        "  上游      {} 个，路由组 {} 个",
        config.providers.len(),
        config.groups.len()
    );
    Ok(())
}

/// 加上游。内置厂商一行搞定：`add deepseek sk-xxx`；免 key 的
/// `add workbuddy` / `add qwenwork`；自定义的
/// `add 七牛云 --url https://api.qnaigc.com/v1 --key {{API_KEY_…}}`
/// （协议默认 openai_chat，`--protocol` 可换）。
async fn add(rest: &[&str]) -> anyhow::Result<()> {
    let mut preset: Option<String> = None;
    let mut key: Option<String> = None;
    let mut name: Option<String> = None;
    let mut url: Option<String> = None;
    let mut protocol: Option<String> = None;
    let mut models: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--url" => {
                url = rest.get(i + 1).map(|s| s.trim().to_string());
                i += 2;
            }
            "--models" => {
                let raw = rest.get(i + 1).copied().unwrap_or_default();
                models = raw
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                i += 2;
            }
            "--name" => {
                name = rest.get(i + 1).map(|s| s.to_string());
                i += 2;
            }
            "--key" => {
                key = rest.get(i + 1).map(|s| clean_key(s));
                i += 2;
            }
            "--protocol" | "--proto" => {
                protocol = rest.get(i + 1).map(|s| s.to_string());
                i += 2;
            }
            word => {
                // The first bare word is the preset id (or the custom
                // name); a second bare word rides as the key.
                if preset.is_none() {
                    preset = Some(word.to_string());
                } else if key.is_none() {
                    key = Some(clean_key(word));
                }
                i += 1;
            }
        }
    }
    let Some(preset) = preset else {
        anyhow::bail!(
            "用法：\n  add deepseek sk-xxx     （内置厂商）\n  add workbuddy / add qwenwork   （免 key，用本机登录态）\n  add 七牛云 --url https://api.qnaigc.com/v1 --key xxx\n                          （自定义，协议默认 openai_chat，\n                           --protocol anthropic / gemini / openai_responses 可换）"
        );
    };
    // 自定义上游（名字不在内置预设里）需要 --url 或 --name；协议可选。
    let protocol_value = protocol.as_deref().map(clean_protocol);
    if presets::get(&preset).is_none() && (url.is_some() || name.is_some()) {
        let name = name.clone().unwrap_or_else(|| preset.clone());
        let id = slug(&name);
        super::provider_cmd(crate::ProviderCmd::Add {
            preset: "custom".into(),
            key,
            name: Some(name),
            url,
            models,
        })
        .await?;
        // 协议不是默认值时补一条设置（Add 没有 protocol 参数，用 update）。
        if let Some(p) = protocol_value {
            set_protocol(&id, p).await?;
        }
        return Ok(());
    }
    if let Some(p) = protocol_value {
        anyhow::bail!(
            "内置厂商的协议是预设好的（{preset}）。--protocol 只用于自定义上游：\n  add <名字> --url <地址> --protocol {p}"
        );
    }
    super::provider_cmd(crate::ProviderCmd::Add {
        preset,
        key,
        name,
        url,
        models,
    })
    .await
}

/// `openai` / `anthropic` / `claude` / `gemini` / `responses` / `chat` …
/// 都收，归一成网关认的协议名。
fn clean_protocol(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "anthropic" | "claude" | "messages" => "anthropic".into(),
        "gemini" | "google" => "gemini".into(),
        "responses" | "openai_responses" | "openai-responses" => "openai_responses".into(),
        _ => "openai_chat".into(), // openai / chat / openai_chat / 其它一律按 OpenAI 兼容
    }
}

/// 清洗用户粘贴的密钥：去掉中文引号、直引号与空白。
///
/// 用户常从文档或聊天记录里复制，密钥外面可能带着 " " 「」 或全角空格。
fn clean_key(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '“' || c == '”' || c == '‘' || c == '’')
        .trim()
        .to_string()
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        // Unicode letters count too: a Chinese vendor name ("七牛云") must
        // not collapse into an empty slug.
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "custom".to_string()
    } else {
        out
    }
}

/// 给刚建的自定义上游设置协议（Add 命令本身不带 protocol）。
async fn set_protocol(id: &str, protocol: String) -> anyhow::Result<()> {
    let parsed = match protocol.as_str() {
        "anthropic" => crate::protocol::ir::Protocol::Anthropic,
        "gemini" => crate::protocol::ir::Protocol::Gemini,
        "openai_responses" => crate::protocol::ir::Protocol::OpenAiResponses,
        _ => crate::protocol::ir::Protocol::OpenAiChat,
    };
    let config = load()?;
    let mut config = config;
    {
        let Some(p) = config.provider_mut(id) else {
            return Ok(()); // provider 刚建好，理论上不会缺
        };
        p.protocol = parsed;
    }
    Store::new().save(&config)?;
    println!("  protocol    {protocol}");
    Ok(())
}

/// WorkBuddy：login / status / credits / models / checkin / accounts / use。
async fn workbuddy(rest: &[&str]) -> anyhow::Result<()> {
    let sub = rest.first().copied().unwrap_or("status");
    match sub {
        "login" => {
            // `workbuddy login [名字]`：不给名字就叫 default；给了就是账号名。
            // `--no-open` 只打印链接，不去开浏览器（远程 shell / 无桌面环境）。
            let mut name = "default".to_string();
            let mut no_open = false;
            for word in rest.iter().skip(1) {
                match *word {
                    "--no-open" | "--noopen" => no_open = true,
                    other => name = other.to_string(),
                }
            }
            super::workbuddy_cmd(crate::WorkBuddyCmd::Login { account: name, no_open }).await
        }
        "status" | "s" => super::workbuddy_cmd(crate::WorkBuddyCmd::Status).await,
        "credits" | "credit" => super::workbuddy_cmd(crate::WorkBuddyCmd::Credits).await,
        "models" | "m" => super::workbuddy_cmd(crate::WorkBuddyCmd::Models).await,
        "checkin" => super::workbuddy_cmd(crate::WorkBuddyCmd::Checkin).await,
        "accounts" | "acc" => super::workbuddy_cmd(crate::WorkBuddyCmd::Accounts).await,
        "use" => {
            let Some(name) = rest.get(1) else {
                anyhow::bail!("切换到哪个账号？workbuddy use <名字>（workbuddy accounts 看有哪些）");
            };
            super::workbuddy_cmd(crate::WorkBuddyCmd::Use {
                name: name.to_string(),
            })
            .await
        }
        other => anyhow::bail!(
            "workbuddy 不认识「{other}」。可用：login / status / credits / models / checkin / accounts / use"
        ),
    }
}

/// QwenWork：login / status / credits / models。
async fn qwen(rest: &[&str]) -> anyhow::Result<()> {
    let sub = rest.first().copied().unwrap_or("status");
    match sub {
        "login" => {
            let no_open = rest
                .iter()
                .skip(1)
                .any(|w| *w == "--no-open" || *w == "--noopen");
            super::qwen_cmd(crate::QwenCmd::Login { no_open }).await
        }
        "status" | "s" => super::qwen_cmd(crate::QwenCmd::Status).await,
        "credits" | "credit" => super::qwen_cmd(crate::QwenCmd::Credits).await,
        "models" | "m" => super::qwen_cmd(crate::QwenCmd::Models).await,
        other => anyhow::bail!("qwen 不认识「{other}」。可用：login / status / credits / models"),
    }
}

/// 建组：`group backup deepseek/deepseek-chat,glm/glm-4.6`（order 主备）。
async fn group(rest: &[&str]) -> anyhow::Result<()> {
    let Some(name) = rest.first() else {
        anyhow::bail!("用法：group backup deepseek/deepseek-chat,glm/glm-4.6");
    };
    let models: Vec<String> = rest
        .get(1..)
        .unwrap_or(&[])
        .iter()
        .flat_map(|s| s.split(','))
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .collect();
    if models.is_empty() {
        anyhow::bail!("组里要有成员；例如 group backup deepseek/deepseek-chat");
    }
    super::group_cmd(crate::GroupCmd::Add {
        name: name.to_string(),
        models,
        routing: "order".into(),
        stays: "auto".into(),
    })
    .await
}

/// `use claude deepseek/deepseek-chat` — 把某个 agent 指到一个模型。
async fn use_agent(rest: &[&str]) -> anyhow::Result<()> {
    let (Some(agent), Some(model)) = (rest.first(), rest.get(1)) else {
        anyhow::bail!(
            "用法：use claude deepseek/deepseek-chat（agent 是 claude/codex/gemini/opencode）"
        );
    };
    let args = crate::AgentModelArgs {
        model: model.to_string(),
    };
    super::set_agent(agent, args).await
}

/// 在默认浏览器里打开控制台。
async fn open_console() -> anyhow::Result<()> {
    let config = load()?;
    let s = &config.settings;
    let url = format!("http://{}:{}", s.gateway_addr, s.gateway_port);
    println!("  打开 {url}");
    let mut opened = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", url.as_str()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    } else {
        std::process::Command::new("open")
            .arg(url.as_str())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
    let _ = opened.as_mut().map(|child| child.wait());
    Ok(())
}

fn load() -> anyhow::Result<Config> {
    Store::new().load()
}
