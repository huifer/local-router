//! QwenWork account-context and wallet parsing — the personal plan's quota.

use serde::Serialize;

/// One credit pool QwenWork's client reads from the wallets endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct CreditPool {
    pub name: String,
    pub balance: f64,
}

/// The account's quota and balance, as the dashboard reads it.
#[derive(Debug, Clone, Serialize)]
pub struct AccountQuota {
    pub plan: String,
    /// What is left, as a balance string (the personal plan only reports
    /// `remaining`, so this is the headline number).
    pub balance: String,
    /// When the plan resets, RFC 3339.
    pub resets_at: Option<String>,
    /// Each wallet pool (daily / monthly / long-term), when present.
    pub pools: Vec<CreditPool>,
    /// A human summary, like `980 / 1000 credits`.
    pub display: String,
    /// Whether the account is out of credits.
    pub empty: bool,
}

/// The unwrap the client does: the envelope's `data` object, else the
/// envelope itself.
fn unwrap(v: &serde_json::Value) -> &serde_json::Value {
    if let Some(data) = v.get("data") {
        if data.is_object() && !data.is_array() {
            return data;
        }
    }
    v
}

fn finite(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64().filter(|x| x.is_finite()),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok().filter(|x| x.is_finite()),
        _ => None,
    }
}

const POOLS: [(&str, &str); 3] = [
    ("daily_credits", "daily"),
    ("monthly_credits", "monthly"),
    ("longterm_credits", "long-term"),
];

/// The wallet pools, each `total_balance`.
fn wallet_pools(wallets: &serde_json::Value) -> Vec<String> {
    let d = unwrap(wallets);
    let mut out = Vec::new();
    for (key, name) in POOLS {
        if let Some(p) = d.get(key) {
            if let Some(b) = finite(p.get("total_balance").unwrap_or(&serde_json::Value::Null)) {
                if b > 0.0 {
                    out.push(format!("{name} {}", gfmt_local(b)));
                }
            }
        }
    }
    out
}

/// Go's `%g` formatting, used by the client's breakdown display.
fn gfmt_local(n: f64) -> String {
    let s = format!("{:.6e}", n);
    let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let x: i32 = exp.parse().unwrap_or(0);
    if !(-4..6).contains(&x) {
        format!("{mantissa}e{x:0>3}")
    } else if n.fract() == 0.0 {
        format!("{:.0}", n)
    } else {
        format!("{n}")
    }
}

fn to_pool(key: &str, name: &str, wallets: &serde_json::Value) -> Option<CreditPool> {
    let d = unwrap(wallets);
    let b = finite(
        d.get(key)
            .and_then(|p| p.get("total_balance"))
            .unwrap_or(&serde_json::Value::Null),
    )?;
    if b > 0.0 {
        Some(CreditPool {
            name: name.to_string(),
            balance: b,
        })
    } else {
        None
    }
}

/// Parse the account-context (`/api/v1/adapter/user/account-context`) for
/// the plan, quota and the wallets that go with it. The personal public
/// cloud's quota has `total` and `used` null — only `remaining` — so we
/// show the balance; an account with a real total keeps its used/total
/// window.
pub fn parse_account_context(
    ctx: &serde_json::Value,
    wallets: Option<&serde_json::Value>,
) -> AccountQuota {
    let data = unwrap(ctx);
    let q = data.get("quota").filter(|q| q.is_object() && !q.is_array());

    let plan = data
        .get("plan")
        .and_then(|p| p.get("name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("QwenWork")
        .to_string();
    let refresh = data
        .get("plan")
        .and_then(|p| {
            p.get("next_refresh_date")
                .or_else(|| p.get("next_due_date"))
        })
        .and_then(serde_json::Value::as_str);
    let resets_at = refresh.and_then(|r| {
        chrono::DateTime::parse_from_rfc3339(r)
            .ok()
            .map(|t| t.to_rfc3339())
            .or_else(|| {
                // The server also answers "YYYY-MM-DD HH:MM:SS" with no
                // zone; read it as UTC (what `DateTime::parse_from_str`
                // cannot do — that one demands a zone in the format).
                chrono::NaiveDateTime::parse_from_str(r, "%Y-%m-%d %H:%M:%S")
                    .ok()
                    .map(|t| t.and_utc().to_rfc3339())
            })
    });

    let total = q.and_then(|q| finite(&q["total"]));
    let remaining = q.and_then(|q| finite(&q["remaining"]));
    let used = q
        .and_then(|q| finite(&q["used"]))
        .or(match (total, remaining) {
            (Some(t), Some(r)) => Some(t - r),
            _ => None,
        });

    let mut pools: Vec<CreditPool> = Vec::new();
    let mut display = String::new();

    // A real total/used window takes precedence in the headline.
    if let (Some(t), Some(u)) = (total, used) {
        if t > 0.0 && u >= 0.0 {
            display = format!("{} / {} credits", gfmt_local(u), gfmt_local(t));
            pools.push(CreditPool {
                name: "Credits".to_string(),
                balance: t - u,
            });
        }
    }

    let mut empty = false;
    if let Some(r) = remaining {
        let names = wallets.map(wallet_pools).unwrap_or_default();
        if let Some(w) = wallets {
            for (key, name) in POOLS {
                if let Some(p) = to_pool(key, name, w) {
                    pools.push(p);
                }
            }
        }
        let balance = format!(
            "{} credits{}",
            gfmt_local(r),
            if names.is_empty() {
                String::new()
            } else {
                format!(" ({})", names.join(" · "))
            }
        );
        if display.is_empty() {
            display = balance;
        }
        empty = r <= 0.0;
    }

    if display.is_empty() {
        display = "the account reports no credits".to_string();
    }

    AccountQuota {
        plan,
        balance: display.clone(),
        resets_at,
        pools,
        display,
        empty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_personal_account_shows_its_remaining_balance() {
        // The personal cloud's quota carries only `remaining`.
        let ctx = json!({
            "plan": {"name": "QwenWork Pro", "next_refresh_date": "2026-11-01 00:00:00"},
            "quota": {"total": null, "used": null, "remaining": 980.5}
        });
        let quota = parse_account_context(&ctx, None);
        assert_eq!(quota.plan, "QwenWork Pro");
        assert!(quota.balance.contains("980.5"));
        assert!(!quota.empty);
        assert!(quota.resets_at.is_some());
    }

    #[test]
    fn a_real_total_used_window_becomes_the_headline() {
        let ctx = json!({
            "plan": {"name": "Team"},
            "quota": {"total": 1000.0, "used": 250.0}
        });
        let quota = parse_account_context(&ctx, None);
        assert!(quota.display.contains("250"));
        assert!(quota.display.contains("1000"));
        assert!(!quota.empty);
        // The headline window lands as the first pool.
        assert_eq!(quota.pools[0].name, "Credits");
        assert!((quota.pools[0].balance - 750.0).abs() < 1e-9);
    }

    #[test]
    fn wallet_pools_are_listed_with_their_balances() {
        let ctx = json!({"quota": {"remaining": 500.0}});
        let wallets = json!({
            "daily_credits": {"total_balance": 120.0},
            "monthly_credits": {"total_balance": 0.0},
            "longterm_credits": {"total_balance": 88.0}
        });
        let quota = parse_account_context(&ctx, Some(&wallets));
        // Zero-balance pools are dropped; the names ride the balance line.
        let names: Vec<&str> = quota.pools.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["daily", "long-term"]);
        assert!(quota.balance.contains("daily 120"));
        assert!(quota.balance.contains("long-term 88"));
    }

    #[test]
    fn an_empty_account_is_flagged() {
        let ctx = json!({"quota": {"remaining": 0.0}});
        let quota = parse_account_context(&ctx, None);
        assert!(quota.empty);
    }

    #[test]
    fn a_shell_envelope_is_unwrapped_but_a_list_is_not() {
        // {"data": {...}} unwraps to the object.
        let wrapped = json!({"data": {"quota": {"remaining": 10.0}}});
        assert!(parse_account_context(&wrapped, None).balance.contains("10"));
        // A data *array* is not an envelope; the object itself is read.
        let listed = json!({"data": [1, 2], "quota": {"remaining": 7.0}});
        assert!(parse_account_context(&listed, None).balance.contains("7"));
        // No quota at all: a graceful, non-empty message.
        let bare = json!({"plan": {"name": "QwenWork"}});
        let quota = parse_account_context(&bare, None);
        assert!(!quota.display.is_empty());
    }

    #[test]
    fn string_balances_and_garbage_do_not_break_the_parse() {
        let ctx = json!({"quota": {"remaining": "42.5"}});
        assert!(parse_account_context(&ctx, None).balance.contains("42.5"));
        let garbage = json!({"quota": {"remaining": "soon"}});
        let quota = parse_account_context(&garbage, None);
        assert!(!quota.empty);
        assert!(!quota.display.is_empty());
    }
}
