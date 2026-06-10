//! `checkout.session` (spec §10) — object + create/retrieve/list only.
//!
//! The session's `url` points at zebrafish's own hosted checkout page
//! (`http://<host>/checkout/{cs_id}`); the page itself and the confirm flow
//! land in WS-G, completion cascades in WS-D/F.

use serde_json::{Value, json};
use zebrafish_core::{World, faker};

use crate::error::StripeError;
use crate::resource::{
    CrudEvents, RequestMeta, Resource, as_i64, metadata_of, missing_param, missing_reference,
};

/// Sessions a buyer never reaches expire after 24 virtual hours.
const EXPIRY_SECONDS: i64 = 24 * 60 * 60;

/// The `checkout.session` resource.
#[derive(Debug)]
pub struct CheckoutSession;

impl Resource for CheckoutSession {
    fn type_name(&self) -> &'static str {
        "checkout.session"
    }

    fn id_prefix(&self) -> &'static str {
        "cs"
    }

    fn plural(&self) -> &'static str {
        "checkout/sessions"
    }

    fn supports_update(&self) -> bool {
        false
    }

    fn supports_delete(&self) -> bool {
        false
    }

    fn crud_events(&self) -> CrudEvents {
        CrudEvents::NONE // checkout.session.completed is emitted by WS-G/WS-D.
    }

    fn validate_create(&self, body: &Value) -> Result<(), StripeError> {
        match body.get("mode").and_then(Value::as_str) {
            None | Some("") => Err(missing_param("mode")),
            Some("payment" | "subscription" | "setup") => Ok(()),
            Some(other) => {
                let mut e = StripeError::invalid_request(format!(
                    "Invalid mode: '{other}' must be one of payment, subscription, or setup"
                ));
                e.param = Some("mode".to_string());
                Err(e)
            }
        }
    }

    fn default_state(
        &self,
        body: &Value,
        world: &mut World,
        meta: &RequestMeta,
    ) -> Result<Value, StripeError> {
        if let Some(customer) = body.get("customer").and_then(Value::as_str) {
            let exists = world
                .get_live_object(customer)?
                .is_some_and(|c| c.get("object").and_then(Value::as_str) == Some("customer"));
            if !exists {
                return Err(missing_reference("customer", customer, "customer"));
            }
        }

        // Total up referenced prices; the line items themselves are not part of
        // the session payload (Stripe serves them via a separate sub-list).
        let mut amount: Option<i64> = None;
        let mut currency: Option<String> = None;
        if let Some(items) = body.get("line_items").and_then(Value::as_array) {
            for item in items {
                let Some(price_id) = item.get("price").and_then(Value::as_str) else {
                    return Err(missing_param("line_items[0][price]"));
                };
                let price = world
                    .get_live_object(price_id)?
                    .filter(|p| p.get("object").and_then(Value::as_str) == Some("price"))
                    .ok_or_else(|| missing_reference("price", price_id, "line_items[0][price]"))?;
                let quantity = item.get("quantity").and_then(as_i64).unwrap_or(1);
                let unit = price
                    .get("unit_amount")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                amount = Some(amount.unwrap_or(0) + unit * quantity);
                if currency.is_none() {
                    currency = price
                        .get("currency")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
        }

        let id = world.new_checkout_session_id();
        let created = world.now();
        let client_secret = faker::client_secret(world.rng(), &id);
        let url = format!("http://{}/checkout/{}", meta.host, id);

        Ok(json!({
            "id": id,
            "object": "checkout.session",
            "adaptive_pricing": null,
            "after_expiration": null,
            "allow_promotion_codes": null,
            "amount_subtotal": amount,
            "amount_total": amount,
            "automatic_tax": { "enabled": false, "liability": null, "provider": null, "status": null },
            "billing_address_collection": null,
            "cancel_url": body.get("cancel_url").and_then(Value::as_str),
            "client_reference_id": body.get("client_reference_id").and_then(Value::as_str),
            "client_secret": client_secret,
            "collected_information": null,
            "consent": null,
            "consent_collection": null,
            "created": created,
            "currency": currency,
            "currency_conversion": null,
            "custom_fields": [],
            "custom_text": {
                "after_submit": null,
                "shipping_address": null,
                "submit": null,
                "terms_of_service_acceptance": null,
            },
            "customer": body.get("customer").and_then(Value::as_str),
            "customer_account": null,
            "customer_creation": "if_required",
            "customer_details": null,
            "customer_email": body.get("customer_email").and_then(Value::as_str),
            "discounts": [],
            "expires_at": created + EXPIRY_SECONDS,
            "invoice": null,
            "invoice_creation": null,
            "livemode": false,
            "locale": null,
            "metadata": metadata_of(body),
            "mode": body["mode"],
            "origin_context": null,
            "payment_intent": null,
            "payment_link": null,
            "payment_method_collection": "always",
            "payment_method_configuration_details": null,
            "payment_method_options": {},
            "payment_method_types": ["card"],
            "payment_status": "unpaid",
            "permissions": null,
            "phone_number_collection": { "enabled": false },
            "recovered_from": null,
            "saved_payment_method_options": null,
            "setup_intent": null,
            "shipping_address_collection": null,
            "shipping_cost": null,
            "shipping_options": [],
            "status": "open",
            "submit_type": null,
            "subscription": null,
            "success_url": body.get("success_url").and_then(Value::as_str),
            "total_details": { "amount_discount": 0, "amount_shipping": 0, "amount_tax": 0 },
            "ui_mode": "hosted",
            "url": url,
            "wallet_options": null,
        }))
    }
}
