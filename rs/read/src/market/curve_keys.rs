//! Exact sparse keys, or compact axes only when the full Cartesian grid exists.
use crate::Json;
use std::collections::HashSet;

pub fn complete_grid(keys: &Json) -> bool {
    let Some(rows) = keys.as_array() else {
        return false;
    };
    let mut products = HashSet::new();
    let mut points = HashSet::new();
    let mut windows = HashSet::new();
    let mut tuples = HashSet::new();
    for row in rows {
        let Some(product) = row["market_product"].as_str() else {
            return false;
        };
        let Some(point) = row["delivery_point_id"].as_str() else {
            return false;
        };
        let Some(window) = row["availability_window"].as_str() else {
            return false;
        };
        products.insert(product);
        points.insert(point);
        windows.insert(window);
        tuples.insert((product, point, window));
    }
    !tuples.is_empty() && tuples.len() == products.len() * points.len() * windows.len()
}

pub const FILTER: &str = "(($3::boolean AND s.market_product IN (SELECT market_product FROM requested_keys) \
    AND s.delivery_point_id IN (SELECT delivery_point_id FROM requested_keys) \
    AND s.availability_window IN (SELECT availability_window FROM requested_keys)) \
    OR (NOT $3::boolean AND EXISTS (SELECT 1 FROM requested_keys k \
    WHERE k.market_product = s.market_product AND k.delivery_point_id = s.delivery_point_id \
    AND k.availability_window = s.availability_window)))";
