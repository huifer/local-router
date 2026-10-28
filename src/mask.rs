//! Masking for names and ids the console shows.
//!
//! The console names the person behind every account (provider cards, the
//! quota panel, account pools). Screenshots of it travel — into issues,
//! chats, notes — so the holder's name and their uid are masked at the
//! source, in the gateway's own responses, not left to whatever the
//! frontend happens to render.

/// A person's name: the first character stays, the rest becomes asterisks.
///
/// `张三` → `张*`, `voila` → `v***`. An empty name stays empty.
pub fn person(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return String::new();
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap_or('*');
    let rest = chars.count().clamp(1, 3);
    format!("{first}{}", "*".repeat(rest))
}

/// An identifier (uid, device id): enough to tell two accounts apart, not
/// enough to name the person. `47d438f8-…` → `47d4…`.
pub fn id(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(4).collect();
    let tail = if chars.next().is_some() { "…" } else { "" };
    format!("{head}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keep_their_first_character_only() {
        assert_eq!(person("张三"), "张*");
        assert_eq!(person("voila"), "v***");
        assert_eq!(person("冉伟DVAi"), "冉***");
        // One character cannot be shortened further.
        assert_eq!(person("王"), "王*");
        // Whitespace and emptiness stay empty.
        assert_eq!(person("  "), "");
        assert_eq!(person(""), "");
    }

    #[test]
    fn ids_keep_enough_to_tell_accounts_apart() {
        assert_eq!(id("47d438f8-b0ae-484d-9a48-027d894c9796"), "47d4…");
        assert_eq!(id("abc"), "abc");
        assert_eq!(id(""), "");
    }
}
