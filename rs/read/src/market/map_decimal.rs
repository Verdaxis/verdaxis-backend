//! Compact map economics share the public-market amount operations.
use crate::market::amount::{Amount, sum_quantities};
use std::cmp::Ordering;

pub fn compare(left: &str, right: &str) -> Result<Ordering, String> {
    Ok(Amount::parse(left)?.compare(&Amount::parse(right)?))
}

pub fn add(left: &str, right: &str) -> Result<String, String> {
    sum_quantities(left, right)
}
