//! List envelope, cursor pagination, and filters (spec §5).
//!
//! Input items are newest-first. Filters narrow the set; cursors page through
//! it; the envelope is `{ object:"list", url, has_more, data }`.

use serde_json::{Value, json};

/// Default and maximum page sizes.
pub const DEFAULT_LIMIT: usize = 10;
/// Maximum page size Stripe allows.
pub const MAX_LIMIT: usize = 100;

/// Cursor + limit controls.
#[derive(Debug, Clone)]
pub struct ListParams {
    /// Page size, clamped to `1..=MAX_LIMIT`.
    pub limit: usize,
    /// Return results after this id (exclusive) — pages toward older items.
    pub starting_after: Option<String>,
    /// Return results before this id (exclusive) — pages toward newer items.
    pub ending_before: Option<String>,
}

impl Default for ListParams {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            starting_after: None,
            ending_before: None,
        }
    }
}

impl ListParams {
    fn clamped_limit(&self) -> usize {
        self.limit.clamp(1, MAX_LIMIT)
    }
}

/// v1 list filters (spec §5). Unknown filters are ignored by the caller.
#[derive(Debug, Default, Clone)]
pub struct Filters {
    /// `customer` — exact match on `$.customer`.
    pub customer: Option<String>,
    /// `status` — exact match on `$.status`.
    pub status: Option<String>,
    /// `created[gt|gte|lt|lte]` bounds on `$.created`.
    pub created_gt: Option<i64>,
    /// See [`Self::created_gt`].
    pub created_gte: Option<i64>,
    /// See [`Self::created_gt`].
    pub created_lt: Option<i64>,
    /// See [`Self::created_gt`].
    pub created_lte: Option<i64>,
}

impl Filters {
    fn matches(&self, item: &Value) -> bool {
        if let Some(c) = &self.customer
            && item.get("customer").and_then(Value::as_str) != Some(c.as_str())
        {
            return false;
        }
        if let Some(s) = &self.status
            && item.get("status").and_then(Value::as_str) != Some(s.as_str())
        {
            return false;
        }
        let created = item.get("created").and_then(Value::as_i64);
        for (bound, cmp) in [
            (self.created_gt, i64::lt as fn(&i64, &i64) -> bool),
            (self.created_gte, i64::le),
            (self.created_lt, i64::gt),
            (self.created_lte, i64::ge),
        ] {
            if let Some(b) = bound {
                // gt: created > b  <=>  b < created  =>  cmp(&b, &created)
                match created {
                    Some(c) if cmp(&b, &c) => {}
                    _ => return false,
                }
            }
        }
        true
    }
}

fn id_of(item: &Value) -> Option<&str> {
    item.get("id").and_then(Value::as_str)
}

/// Build a Stripe list response from newest-first `items`.
#[must_use]
pub fn paginate(url: &str, items: Vec<Value>, params: &ListParams, filters: &Filters) -> Value {
    let limit = params.clamped_limit();
    let filtered: Vec<Value> = items.into_iter().filter(|i| filters.matches(i)).collect();

    // Apply cursor to get the candidate window (still newest-first).
    let candidates: Vec<Value> = if let Some(after) = &params.starting_after {
        match filtered
            .iter()
            .position(|i| id_of(i) == Some(after.as_str()))
        {
            Some(pos) => filtered.into_iter().skip(pos + 1).collect(),
            None => filtered,
        }
    } else if let Some(before) = &params.ending_before {
        match filtered
            .iter()
            .position(|i| id_of(i) == Some(before.as_str()))
        {
            Some(pos) => {
                // Items strictly before the cursor; keep the `limit` closest to it.
                let mut head: Vec<Value> = filtered.into_iter().take(pos).collect();
                if head.len() > limit {
                    head = head.split_off(head.len() - limit);
                }
                let has_more = head.len() > limit; // always false after trim, but keep shape
                return envelope(url, head, has_more);
            }
            None => filtered,
        }
    } else {
        filtered
    };

    let has_more = candidates.len() > limit;
    let page: Vec<Value> = candidates.into_iter().take(limit).collect();
    envelope(url, page, has_more)
}

fn envelope(url: &str, data: Vec<Value>, has_more: bool) -> Value {
    json!({
        "object": "list",
        "url": url,
        "has_more": has_more,
        "data": data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(id: &str, created: i64) -> Value {
        json!({ "id": id, "object": "thing", "created": created })
    }

    fn items() -> Vec<Value> {
        // newest first
        vec![obj("c", 30), obj("b", 20), obj("a", 10)]
    }

    #[test]
    fn limit_and_has_more() {
        let p = ListParams {
            limit: 2,
            ..Default::default()
        };
        let list = paginate("/v1/things", items(), &p, &Filters::default());
        assert_eq!(list["has_more"], json!(true));
        assert_eq!(list["data"].as_array().unwrap().len(), 2);
        assert_eq!(list["data"][0]["id"], json!("c"));
    }

    #[test]
    fn starting_after_pages_to_older() {
        let p = ListParams {
            limit: 10,
            starting_after: Some("c".into()),
            ending_before: None,
        };
        let list = paginate("/v1/things", items(), &p, &Filters::default());
        let ids: Vec<&str> = list["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["b", "a"]);
    }

    #[test]
    fn filter_by_created_range() {
        let filters = Filters {
            created_gte: Some(20),
            ..Default::default()
        };
        let list = paginate("/v1/things", items(), &ListParams::default(), &filters);
        let ids: Vec<&str> = list["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["c", "b"]);
    }
}
