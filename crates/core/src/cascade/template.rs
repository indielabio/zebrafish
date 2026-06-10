//! The cascade template language (spec §7.2) — deliberately tiny.
//!
//! Inside any JSON string value a fixture may write `{{expr}}` where `expr` is
//! one of exactly five forms:
//!
//! ```text
//! {{binding.path.0.field}}              value lookup (numeric segments index arrays)
//! {{now}} / {{now + 30d}} / {{now + price.interval}}   clock arithmetic
//! {{faker.email(customer.name)}}        whitelisted faker call
//! {{helpers.subscription_total(subscription)}}         named engine helper (#28)
//! {{id:in}}                             fresh id from the world RNG
//! ```
//!
//! There are **no conditionals and no loops** — anything beyond this grammar is
//! a parse error. A string that is *exactly* one expression substitutes the
//! evaluated value with its native JSON type (`"{{now + 30d}}"` becomes a
//! number); an expression embedded in a longer string must evaluate to a
//! scalar. Unresolvable paths are hard errors: a fixture referencing a missing
//! binding is a fixture bug, not a soft miss.

use serde_json::{Map, Number, Value};

use crate::error::{CoreError, Result};
use crate::rng::WorldRng;
use crate::{faker, id};

use super::helpers;

/// Seconds per billing interval (spec §7: virtual-clock months are 30 days,
/// years 365). Unknown intervals fall back to a month — create-side validation
/// restricts the set, so the fallback is never user-visible.
#[must_use]
pub fn interval_seconds(interval: &str) -> i64 {
    match interval {
        "day" => 86_400,
        "week" => 7 * 86_400,
        "year" => 365 * 86_400,
        _ => 30 * 86_400, // month
    }
}

/// Evaluation environment. Not `&mut World`: the engine evaluates against a
/// snapshot (`bindings`) plus the clock and RNG, which keeps evaluation pure
/// enough to unit-test and sidesteps borrow conflicts in the interpreter.
#[derive(Debug)]
pub struct Env<'a> {
    /// Virtual time the cascade runs at.
    pub now: i64,
    /// The world RNG — `{{id:..}}` and `{{faker...}}` draw from it.
    pub rng: &'a mut WorldRng,
    /// Trigger context entries plus earlier `bind`s, one flat namespace.
    pub bindings: &'a Map<String, Value>,
}

/// A parsed `{{...}}` expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// `binding.path` lookup.
    Path(String),
    /// `now` with an optional `+`/`-` offset.
    Now {
        /// `+1` or `-1`.
        sign: i64,
        /// `None` for bare `{{now}}`.
        operand: Option<Operand>,
    },
    /// `faker.<fn>(args…)`.
    Faker {
        /// Whitelisted function name.
        name: String,
        /// Call arguments.
        args: Vec<Arg>,
    },
    /// `helpers.<fn>(args…)` (#28).
    Helper {
        /// Helper name (see [`super::helpers`]).
        name: String,
        /// Call arguments.
        args: Vec<Arg>,
    },
    /// `id:<prefix>` — a fresh 24-char base62 id.
    Id {
        /// The id prefix, e.g. `"in"`.
        prefix: String,
    },
}

/// The offset operand of a `now` expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    /// A duration literal (`30d`, `12h`, `5m`, `45s`), stored as seconds.
    Seconds(i64),
    /// A path; a string resolves as a symbolic interval (`"month"` → 30d), a
    /// number as raw seconds.
    Path(String),
}

/// One argument to a faker/helper call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    /// A single-quoted literal: `'usd'`.
    Lit(String),
    /// A dotted path into the bindings.
    Path(String),
}

impl std::fmt::Display for Expr {
    /// Canonical form — `parse(expr.to_string()) == expr` (proptested).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(p) => write!(f, "{p}"),
            Self::Now { sign, operand } => match operand {
                None => write!(f, "now"),
                Some(op) => {
                    let s = if *sign >= 0 { '+' } else { '-' };
                    match op {
                        Operand::Seconds(n) => write!(f, "now {s} {n}s"),
                        Operand::Path(p) => write!(f, "now {s} {p}"),
                    }
                }
            },
            Self::Faker { name, args } => write!(f, "faker.{name}({})", join_args(args)),
            Self::Helper { name, args } => write!(f, "helpers.{name}({})", join_args(args)),
            Self::Id { prefix } => write!(f, "id:{prefix}"),
        }
    }
}

fn join_args(args: &[Arg]) -> String {
    args.iter()
        .map(|a| match a {
            Arg::Lit(s) => format!("'{s}'"),
            Arg::Path(p) => p.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// --- parsing -------------------------------------------------------------------

fn is_path(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|seg| {
            !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

fn parse_duration(s: &str) -> Option<i64> {
    let (digits, unit) = s.split_at(s.len().checked_sub(1)?);
    let n: i64 = digits.parse().ok()?;
    let mult = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => return None,
    };
    Some(n * mult)
}

fn parse_args(inner: &str) -> Result<Vec<Arg>> {
    let inner = inner.trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    inner
        .split(',')
        .map(|raw| {
            let raw = raw.trim();
            if let Some(stripped) = raw.strip_prefix('\'') {
                let lit = stripped
                    .strip_suffix('\'')
                    .ok_or_else(|| bad(&format!("unterminated literal {raw}")))?;
                Ok(Arg::Lit(lit.to_string()))
            } else if is_path(raw) {
                Ok(Arg::Path(raw.to_string()))
            } else {
                Err(bad(&format!("invalid argument '{raw}'")))
            }
        })
        .collect()
}

fn parse_call(expr: &str) -> Result<(String, Vec<Arg>)> {
    let open = expr
        .find('(')
        .ok_or_else(|| bad(&format!("expected '(...)' in '{expr}'")))?;
    let close = expr
        .rfind(')')
        .filter(|c| *c == expr.len() - 1)
        .ok_or_else(|| bad(&format!("expected trailing ')' in '{expr}'")))?;
    let name = expr[..open].trim();
    if !is_path(name) || name.contains('.') {
        return Err(bad(&format!("invalid function name '{name}'")));
    }
    Ok((name.to_string(), parse_args(&expr[open + 1..close])?))
}

fn bad(msg: &str) -> CoreError {
    CoreError::Cascade(format!("template: {msg}"))
}

/// Parse the text between `{{` and `}}`.
pub fn parse(input: &str) -> Result<Expr> {
    let expr = input.trim();

    if let Some(prefix) = expr.strip_prefix("id:") {
        if !is_path(prefix) || prefix.contains('.') {
            return Err(bad(&format!("invalid id prefix '{prefix}'")));
        }
        return Ok(Expr::Id {
            prefix: prefix.to_string(),
        });
    }

    if let Some(rest) = expr.strip_prefix("faker.") {
        let (name, args) = parse_call(rest)?;
        return Ok(Expr::Faker { name, args });
    }

    if let Some(rest) = expr.strip_prefix("helpers.") {
        let (name, args) = parse_call(rest)?;
        return Ok(Expr::Helper { name, args });
    }

    if expr == "now" {
        return Ok(Expr::Now {
            sign: 1,
            operand: None,
        });
    }
    if let Some(rest) = expr.strip_prefix("now") {
        let rest = rest.trim_start();
        let (sign, operand) = if let Some(op) = rest.strip_prefix('+') {
            (1, op.trim())
        } else if let Some(op) = rest.strip_prefix('-') {
            (-1, op.trim())
        } else {
            return Err(bad(&format!("expected '+' or '-' after now in '{expr}'")));
        };
        let operand = if let Some(secs) = parse_duration(operand) {
            Operand::Seconds(secs)
        } else if is_path(operand) {
            Operand::Path(operand.to_string())
        } else {
            return Err(bad(&format!("invalid now-offset '{operand}'")));
        };
        return Ok(Expr::Now {
            sign,
            operand: Some(operand),
        });
    }

    if is_path(expr) {
        return Ok(Expr::Path(expr.to_string()));
    }
    Err(bad(&format!("unrecognized expression '{expr}'")))
}

// --- evaluation ------------------------------------------------------------------

/// Resolve a dotted path against the bindings. Numeric segments index arrays.
/// Misses are hard errors (a fixture bug, spec §7.2).
fn resolve_path<'v>(path: &str, bindings: &'v Map<String, Value>) -> Result<&'v Value> {
    let mut segs = path.split('.');
    let root = segs.next().expect("split yields at least one segment");
    let mut cur = bindings
        .get(root)
        .ok_or_else(|| bad(&format!("unknown binding '{root}' in path '{path}'")))?;
    for seg in segs {
        cur = match cur {
            Value::Object(map) => map.get(seg),
            Value::Array(items) => seg.parse::<usize>().ok().and_then(|i| items.get(i)),
            _ => None,
        }
        .ok_or_else(|| bad(&format!("path '{path}' has no segment '{seg}'")))?;
    }
    Ok(cur)
}

fn resolve_arg(arg: &Arg, bindings: &Map<String, Value>) -> Result<Value> {
    match arg {
        Arg::Lit(s) => Ok(Value::String(s.clone())),
        Arg::Path(p) => Ok(resolve_path(p, bindings)?.clone()),
    }
}

fn arg_str(args: &[Value], i: usize, fn_name: &str) -> Result<String> {
    args.get(i)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            bad(&format!(
                "{fn_name} expects a string argument at position {i}"
            ))
        })
}

/// Evaluate one parsed expression.
pub fn eval(expr: &Expr, env: &mut Env<'_>) -> Result<Value> {
    match expr {
        Expr::Path(p) => Ok(resolve_path(p, env.bindings)?.clone()),

        Expr::Now { sign, operand } => {
            let offset = match operand {
                None => 0,
                Some(Operand::Seconds(n)) => *n,
                Some(Operand::Path(p)) => match resolve_path(p, env.bindings)? {
                    Value::String(interval) => interval_seconds(interval),
                    Value::Number(n) => n
                        .as_i64()
                        .ok_or_else(|| bad(&format!("now-offset path '{p}' is not an integer")))?,
                    other => {
                        return Err(bad(&format!(
                            "now-offset path '{p}' must be an interval string or \
                             seconds, got {other}"
                        )));
                    }
                },
            };
            Ok(Value::Number(Number::from(env.now + sign * offset)))
        }

        Expr::Id { prefix } => Ok(Value::String(id::id(env.rng, prefix))),

        Expr::Faker { name, args } => {
            let args: Vec<Value> = args
                .iter()
                .map(|a| resolve_arg(a, env.bindings))
                .collect::<Result<_>>()?;
            // The whitelist (spec §7.2): greppable, never grows implicitly.
            match name.as_str() {
                "name" => Ok(Value::String(faker::name(env.rng))),
                "email" => {
                    let name = arg_str(&args, 0, "faker.email")?;
                    Ok(Value::String(faker::email(env.rng, &name)))
                }
                "address" => Ok(faker::address(env.rng)),
                "card_fingerprint" => Ok(Value::String(faker::card_fingerprint(env.rng))),
                "client_secret" => {
                    let id = arg_str(&args, 0, "faker.client_secret")?;
                    Ok(Value::String(faker::client_secret(env.rng, &id)))
                }
                "price_amount" => {
                    let currency = arg_str(&args, 0, "faker.price_amount")?;
                    Ok(Value::Number(Number::from(faker::price_amount(
                        env.rng, &currency,
                    ))))
                }
                "last4" => {
                    let pan = arg_str(&args, 0, "faker.last4")?;
                    Ok(Value::String(faker::last4(&pan)))
                }
                "brand_from_pan" => {
                    let pan = arg_str(&args, 0, "faker.brand_from_pan")?;
                    Ok(Value::String(faker::brand_from_pan(&pan).to_string()))
                }
                other => Err(bad(&format!("unknown faker function '{other}'"))),
            }
        }

        Expr::Helper { name, args } => {
            let args: Vec<Value> = args
                .iter()
                .map(|a| resolve_arg(a, env.bindings))
                .collect::<Result<_>>()?;
            helpers::call(name, &args, env.now)
        }
    }
}

/// Stringify a scalar for embedded (`"inv-{{id:x}}"`) substitution.
fn embed(value: &Value, expr: &str) -> Result<String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(bad(&format!(
            "'{{{{{expr}}}}}' is embedded in a string but evaluates to a non-scalar"
        ))),
    }
}

/// Substitute `{{...}}` in one string, per the whole-value/embedded rules.
fn render_str(s: &str, env: &mut Env<'_>) -> Result<Value> {
    let Some(start) = s.find("{{") else {
        return Ok(Value::String(s.to_string()));
    };

    // Whole-value form: exactly `{{expr}}` → native type.
    let trimmed = s.trim();
    if trimmed.starts_with("{{") && trimmed.ends_with("}}") && trimmed.matches("{{").count() == 1 {
        let inner = &trimmed[2..trimmed.len() - 2];
        return eval(&parse(inner)?, env);
    }

    // Embedded form: substitute scalars left to right.
    let mut out = String::with_capacity(s.len());
    out.push_str(&s[..start]);
    let mut rest = &s[start..];
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let close = after
            .find("}}")
            .ok_or_else(|| bad(&format!("unterminated '{{{{' in \"{s}\"")))?;
        let inner = &after[..close];
        let value = eval(&parse(inner)?, env)?;
        out.push_str(&embed(&value, inner.trim())?);
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    Ok(Value::String(out))
}

/// Walk a JSON tree, substituting templates in every string value.
pub fn render(value: &Value, env: &mut Env<'_>) -> Result<Value> {
    match value {
        Value::String(s) => render_str(s, env),
        Value::Array(items) => Ok(Value::Array(
            items
                .iter()
                .map(|v| render(v, env))
                .collect::<Result<_>>()?,
        )),
        Value::Object(map) => {
            let mut out = Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), render(v, env)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env_with<'a>(rng: &'a mut WorldRng, bindings: &'a Map<String, Value>) -> Env<'a> {
        Env {
            now: 1_700_000_000,
            rng,
            bindings,
        }
    }

    fn bindings() -> Map<String, Value> {
        json!({
            "subscription": {
                "id": "sub_1",
                "currency": "usd",
                "items": { "data": [ { "price": { "recurring": { "interval": "month" },
                                                  "unit_amount": 2900 } } ] },
            },
            "customer": { "name": "Dana Smith" },
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[test]
    fn whole_value_substitution_keeps_native_types() {
        let b = bindings();
        let mut rng = WorldRng::from_seed(1);
        let mut env = env_with(&mut rng, &b);
        let out = render(
            &json!({
                "at": "{{now}}",
                "renews": "{{now + subscription.items.data.0.price.recurring.interval}}",
                "in_30d": "{{now + 30d}}",
                "amount": "{{subscription.items.data.0.price.unit_amount}}",
            }),
            &mut env,
        )
        .unwrap();
        assert_eq!(out["at"], json!(1_700_000_000));
        assert_eq!(out["renews"], json!(1_700_000_000 + 30 * 86_400));
        assert_eq!(out["in_30d"], json!(1_700_000_000 + 30 * 86_400));
        assert_eq!(out["amount"], json!(2900));
    }

    #[test]
    fn embedded_substitution_stringifies_scalars() {
        let b = bindings();
        let mut rng = WorldRng::from_seed(1);
        let mut env = env_with(&mut rng, &b);
        let out = render(&json!("sub={{subscription.id}} at={{now}}"), &mut env).unwrap();
        assert_eq!(out, json!("sub=sub_1 at=1700000000"));
    }

    #[test]
    fn embedded_non_scalar_is_an_error() {
        let b = bindings();
        let mut rng = WorldRng::from_seed(1);
        let mut env = env_with(&mut rng, &b);
        assert!(render(&json!("x{{subscription.items}}y"), &mut env).is_err());
    }

    #[test]
    fn id_and_faker_draw_from_the_rng() {
        let b = bindings();
        let mut rng = WorldRng::from_seed(7);
        let mut env = env_with(&mut rng, &b);
        let id = render(&json!("{{id:in}}"), &mut env).unwrap();
        assert!(id.as_str().unwrap().starts_with("in_"));
        assert_eq!(id.as_str().unwrap().len(), 3 + 24);

        let email = render(&json!("{{faker.email(customer.name)}}"), &mut env).unwrap();
        assert!(email.as_str().unwrap().contains("@example."));

        let amount = render(&json!("{{faker.price_amount('usd')}}"), &mut env).unwrap();
        assert!(amount.as_i64().unwrap() > 0);
    }

    #[test]
    fn missing_path_is_a_hard_error() {
        let b = bindings();
        let mut rng = WorldRng::from_seed(1);
        let mut env = env_with(&mut rng, &b);
        let err = render(&json!("{{subscription.nope}}"), &mut env).unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
        assert!(render(&json!("{{ghost.id}}"), &mut env).is_err());
    }

    #[test]
    fn no_conditionals_or_loops() {
        for bad in ["{{#if x}}", "{{x | upper}}", "{{for i in xs}}", "{{a b}}"] {
            let b = bindings();
            let mut rng = WorldRng::from_seed(1);
            let mut env = env_with(&mut rng, &b);
            assert!(
                render(&json!(bad), &mut env).is_err(),
                "{bad} must not parse"
            );
        }
    }

    #[test]
    fn non_template_strings_are_untouched() {
        let b = bindings();
        let mut rng = WorldRng::from_seed(1);
        let mut env = env_with(&mut rng, &b);
        let v = json!({ "s": "plain", "n": 5, "b": true, "x": null, "a": ["y"] });
        assert_eq!(render(&v, &mut env).unwrap(), v);
    }

    #[test]
    fn duration_units() {
        assert_eq!(parse_duration("45s"), Some(45));
        assert_eq!(parse_duration("5m"), Some(300));
        assert_eq!(parse_duration("12h"), Some(43_200));
        assert_eq!(parse_duration("30d"), Some(2_592_000));
        assert_eq!(parse_duration("30x"), None);
        assert_eq!(parse_duration("d"), None);
    }

    // --- proptest: canonical print ↔ parse round-trip (spec §16.2) -------------

    use proptest::prelude::*;

    fn path_strategy() -> impl Strategy<Value = String> {
        proptest::collection::vec("[a-z][a-z0-9_]{0,5}", 1..4).prop_map(|segs| segs.join("."))
    }

    fn arg_strategy() -> impl Strategy<Value = Arg> {
        prop_oneof![
            "[a-z0-9_ ]{0,8}".prop_map(Arg::Lit),
            path_strategy().prop_map(Arg::Path),
        ]
    }

    fn expr_strategy() -> impl Strategy<Value = Expr> {
        prop_oneof![
            path_strategy().prop_map(Expr::Path),
            Just(Expr::Now {
                sign: 1,
                operand: None
            }),
            (prop_oneof![Just(1i64), Just(-1i64)], 0i64..10_000_000).prop_map(|(sign, n)| {
                Expr::Now {
                    sign,
                    operand: Some(Operand::Seconds(n)),
                }
            }),
            (prop_oneof![Just(1i64), Just(-1i64)], path_strategy()).prop_map(|(sign, p)| {
                Expr::Now {
                    sign,
                    operand: Some(Operand::Path(p)),
                }
            }),
            (
                "[a-z][a-z0-9_]{0,8}",
                proptest::collection::vec(arg_strategy(), 0..3)
            )
                .prop_map(|(name, args)| Expr::Faker { name, args }),
            (
                "[a-z][a-z0-9_]{0,8}",
                proptest::collection::vec(arg_strategy(), 0..3)
            )
                .prop_map(|(name, args)| Expr::Helper { name, args }),
            "[a-z][a-z0-9_]{0,4}".prop_map(|prefix| Expr::Id { prefix }),
        ]
    }

    proptest! {
        #[test]
        fn print_parse_round_trips(expr in expr_strategy()) {
            let printed = expr.to_string();
            let reparsed = parse(&printed).expect("canonical form must parse");
            prop_assert_eq!(reparsed, expr);
        }

        #[test]
        fn strings_without_braces_render_to_themselves(s in "[^{}]{0,40}") {
            let b = Map::new();
            let mut rng = WorldRng::from_seed(1);
            let mut env = Env { now: 0, rng: &mut rng, bindings: &b };
            let out = render(&Value::String(s.clone()), &mut env).unwrap();
            prop_assert_eq!(out, Value::String(s));
        }

        #[test]
        fn now_arithmetic_is_exact(n in 0i64..10_000_000, now in 0i64..2_000_000_000) {
            let b = Map::new();
            let mut rng = WorldRng::from_seed(1);
            let mut env = Env { now, rng: &mut rng, bindings: &b };
            let plus = eval(&parse(&format!("now + {n}s")).unwrap(), &mut env).unwrap();
            let minus = eval(&parse(&format!("now - {n}s")).unwrap(), &mut env).unwrap();
            prop_assert_eq!(plus, Value::from(now + n));
            prop_assert_eq!(minus, Value::from(now - n));
        }
    }
}
