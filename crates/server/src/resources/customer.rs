//! `customer` (spec §1) — full CRUD with coherent faker defaults (spec §6.4):
//! an omitted name/email/address is generated, and the email matches the name.

use serde_json::{Value, json};
use zebrafish_core::{World, faker};

use crate::error::StripeError;
use crate::resource::{CrudEvents, RequestMeta, Resource, metadata_of};

/// The `customer` resource.
#[derive(Debug)]
pub struct Customer;

impl Resource for Customer {
    fn type_name(&self) -> &'static str {
        "customer"
    }

    fn id_prefix(&self) -> &'static str {
        "cus"
    }

    fn plural(&self) -> &'static str {
        "customers"
    }

    fn crud_events(&self) -> CrudEvents {
        CrudEvents {
            created: Some("customer.created"),
            updated: Some("customer.updated"),
            deleted: Some("customer.deleted"),
        }
    }

    fn validate_create(&self, _body: &Value) -> Result<(), StripeError> {
        Ok(())
    }

    fn default_state(
        &self,
        body: &Value,
        world: &mut World,
        _meta: &RequestMeta,
    ) -> Result<Value, StripeError> {
        let id = world.new_id(self.id_prefix());
        let created = world.now();

        let name = body
            .get("name")
            .and_then(Value::as_str)
            .map_or_else(|| faker::name(world.rng()), str::to_string);
        let email = body
            .get("email")
            .and_then(Value::as_str)
            .map_or_else(|| faker::email(world.rng(), &name), str::to_string);
        let address = body
            .get("address")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| faker::address(world.rng()));
        let invoice_prefix = world.rng().fill_base62(8).to_ascii_uppercase();

        Ok(json!({
            "id": id,
            "object": "customer",
            "address": address,
            "balance": 0,
            "created": created,
            "currency": null,
            "default_source": null,
            "delinquent": false,
            "description": body.get("description").and_then(Value::as_str),
            "discount": null,
            "email": email,
            "invoice_prefix": invoice_prefix,
            "invoice_settings": {
                "custom_fields": null,
                "default_payment_method": null,
                "footer": null,
                "rendering_options": null,
            },
            "livemode": false,
            "metadata": metadata_of(body),
            "name": name,
            "next_invoice_sequence": 1,
            "phone": body.get("phone").and_then(Value::as_str),
            "preferred_locales": [],
            "shipping": null,
            "tax_exempt": "none",
            "test_clock": null,
        }))
    }
}
