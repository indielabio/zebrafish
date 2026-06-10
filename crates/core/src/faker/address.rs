//! A billing-address shaped JSON object (spec §6.4).

use fake::Fake;
use fake::faker::address::raw::{BuildingNumber, CityName, StateAbbr, StreetName, ZipCode};
use fake::locales::EN;
use serde_json::{Value, json};

use crate::rng::WorldRng;

/// A US-shaped address object matching Stripe's `address` sub-resource fields.
pub fn address(rng: &mut WorldRng) -> Value {
    let number: String = BuildingNumber(EN).fake_with_rng(rng.inner());
    let street: String = StreetName(EN).fake_with_rng(rng.inner());
    let city: String = CityName(EN).fake_with_rng(rng.inner());
    let state: String = StateAbbr(EN).fake_with_rng(rng.inner());
    let postal: String = ZipCode(EN).fake_with_rng(rng.inner());

    json!({
        "line1": format!("{number} {street}"),
        "line2": Value::Null,
        "city": city,
        "state": state,
        "postal_code": postal,
        "country": "US",
    })
}
