//! Coherent person data: names and emails that match each other (spec §6.4).

use fake::Fake;
use fake::faker::name::raw::Name;
use fake::locales::EN;

use crate::rng::WorldRng;

/// Only reserved example domains are ever generated (never a real domain).
const DOMAINS: [&str; 3] = ["example.com", "example.net", "example.org"];

/// A realistic full name, e.g. `"Dana Anderson"`.
pub fn name(rng: &mut WorldRng) -> String {
    Name(EN).fake_with_rng::<String, _>(rng.inner())
}

/// An email coherent with `name`, e.g. `"Dana Anderson"` => `"d.anderson@example.net"`.
/// Domains are restricted to the reserved `example.*` set.
pub fn email(rng: &mut WorldRng, name: &str) -> String {
    let mut parts = name.split_whitespace();
    let first = parts.next().unwrap_or("user");
    let last = parts.last().unwrap_or(first);

    let initial = first.chars().next().unwrap_or('u').to_ascii_lowercase();
    let last_clean: String = last
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let last_clean = if last_clean.is_empty() {
        "user".to_string()
    } else {
        last_clean
    };

    let domain = DOMAINS[(rng.below(DOMAINS.len() as u32)) as usize];
    format!("{initial}.{last_clean}@{domain}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_is_coherent_and_reserved() {
        let mut rng = WorldRng::from_seed(3);
        let n = name(&mut rng);
        let e = email(&mut rng, &n);
        let last = n.split_whitespace().last().unwrap().to_ascii_lowercase();
        let last: String = last.chars().filter(char::is_ascii_alphanumeric).collect();
        assert!(e.contains(&last), "{e} should contain {last}");
        assert!(
            DOMAINS.iter().any(|d| e.ends_with(d)),
            "{e} must use a reserved domain"
        );
    }
}
