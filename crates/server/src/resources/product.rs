//! `product` (spec §1) — full CRUD.

use serde_json::{Value, json};
use zebrafish_core::World;

use crate::error::StripeError;
use crate::resource::{CrudEvents, RequestMeta, Resource, as_bool, metadata_of, missing_param};

/// The `product` resource.
#[derive(Debug)]
pub struct Product;

impl Resource for Product {
    fn type_name(&self) -> &'static str {
        "product"
    }

    fn id_prefix(&self) -> &'static str {
        "prod"
    }

    fn plural(&self) -> &'static str {
        "products"
    }

    fn crud_events(&self) -> CrudEvents {
        CrudEvents {
            created: Some("product.created"),
            updated: Some("product.updated"),
            deleted: Some("product.deleted"),
        }
    }

    fn validate_create(&self, body: &Value) -> Result<(), StripeError> {
        if body
            .get("name")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(missing_param("name"));
        }
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
        Ok(json!({
            "id": id,
            "object": "product",
            "active": body.get("active").and_then(as_bool).unwrap_or(true),
            "created": created,
            "default_price": null,
            "description": body.get("description").and_then(Value::as_str),
            "images": body.get("images").cloned().filter(Value::is_array).unwrap_or_else(|| json!([])),
            "livemode": false,
            "marketing_features": [],
            "metadata": metadata_of(body),
            "name": body["name"],
            "package_dimensions": null,
            "shippable": null,
            "statement_descriptor": null,
            "tax_code": null,
            "type": "service",
            "unit_label": null,
            "updated": created,
            "url": body.get("url").and_then(Value::as_str),
        }))
    }
}
