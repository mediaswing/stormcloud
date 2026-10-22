//! What Microsoft Entra ID accepts for a new user, checked before anything is sent, so every way
//! of making a user (the New User form, a CSV import and the onboarding wizard) refuses the same
//! things with the same words.

use crate::i18n::{t, tf};

/// Why Microsoft Entra ID would refuse `alias` as the part of a user principal name before the
/// @, if it would. It also becomes the mail nickname, so it is held to what that can contain.
pub fn alias_problem(alias: &str) -> Option<String> {
    if alias.is_empty() {
        return Some(t("onboard.no_alias"));
    }
    if let Some(c) = alias.chars().find(|c| !(c.is_ascii_alphanumeric() || "'.-_!#^~".contains(*c))) {
        return Some(tf("onboard.alias_char", &[("char", &c)]));
    }
    if alias.starts_with('.') || alias.ends_with('.') || alias.contains("..") {
        return Some(t("onboard.alias_dots"));
    }
    None
}

/// Why Microsoft Entra ID would refuse a new user's password, if it would: it must be 8 to 256
/// characters with three of upper case, lower case, digits and symbols.
pub fn password_problem(p: &str) -> Option<String> {
    let classes = [
        p.chars().any(|c| c.is_ascii_uppercase()),
        p.chars().any(|c| c.is_ascii_lowercase()),
        p.chars().any(|c| c.is_ascii_digit()),
        p.chars().any(|c| !c.is_ascii_alphanumeric()),
    ];
    password_length_problem(p).or_else(|| (classes.iter().filter(|c| **c).count() < 3).then(|| t("onboard.password_classes")))
}

/// Why a password is too short or too long, if it is. A password reset is held only to this: a
/// user can be let off the rest (`DisableStrongPassword`), which the user list doesn't show.
pub fn password_length_problem(p: &str) -> Option<String> {
    (!(8..=256).contains(&p.chars().count())).then(|| t("onboard.password_length"))
}

/// Blank, or a two-letter country code such as GB.
pub fn is_usage_location(s: &str) -> bool {
    s.is_empty() || s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::generate_password;

    #[test]
    fn checks_names_and_passwords() {
        assert_eq!(alias_problem("ann.lee"), None);
        assert!(alias_problem("ann lee").is_some());
        assert!(alias_problem("ann@contoso.com").is_some());
        assert!(alias_problem(".ann").is_some());
        assert!(alias_problem("josé").is_some());
        assert!(password_problem("short1A").is_some());
        assert!(password_problem("alllowercase").is_some());
        assert_eq!(password_problem("Lower-and-1"), None);
        for _ in 0..50 {
            assert_eq!(password_problem(&generate_password()), None);
        }
    }

    #[test]
    fn counts_characters_not_bytes() {
        // Four characters, but eight bytes.
        assert!(password_length_problem("éééé").is_some());
        assert_eq!(password_length_problem("alllowercase"), None);
        assert!(password_length_problem(&"a".repeat(257)).is_some());
    }

    #[test]
    fn usage_locations() {
        assert!(is_usage_location("") && is_usage_location("gb"));
        assert!(!is_usage_location("GBR") && !is_usage_location("G1"));
    }
}
