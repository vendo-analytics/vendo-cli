//! How the TS command code turned JSON values into text, where `output.rs`
//! has no helper for it: template literals (`${value}`), cli-table3 cells and
//! the `??` fallbacks around them. Used by the metrics, models and
//! measurement views (VE-3668).

use serde_json::Value;

use crate::output::{js_string, js_truthy, time_ago};

/// `${value}` in a template literal (or a lone `console.log` argument):
/// `undefined` when the key is missing, `null` for null.
pub fn template(value: Option<&Value>) -> String {
    value.map(js_string).unwrap_or_else(|| "undefined".to_string())
}

/// What cli-table3 prints for a cell: strings, numbers and booleans as
/// `String()` gives them; null, missing, objects and arrays as an empty cell.
pub fn cell(value: Option<&Value>) -> String {
    match value {
        Some(v @ (Value::String(_) | Value::Number(_) | Value::Bool(_))) => js_string(v),
        _ => String::new(),
    }
}

/// The value unless it is null or missing (`value ?? …`).
fn present(value: Option<&Value>) -> Option<&Value> {
    value.filter(|v| !v.is_null())
}

/// `${value ?? fallback}`: an empty string stays empty.
pub fn template_or(value: Option<&Value>, fallback: impl FnOnce() -> String) -> String {
    present(value).map(|v| template(Some(v))).unwrap_or_else(fallback)
}

/// A table cell holding `value ?? fallback`.
pub fn cell_or(value: Option<&Value>, fallback: impl FnOnce() -> String) -> String {
    present(value).map(|v| cell(Some(v))).unwrap_or_else(fallback)
}

/// The TS `timeAgo(value)`: a dimmed dash for a falsy value, otherwise the
/// shared `time_ago` of its text.
pub fn time_ago_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => time_ago(Some(s)),
        Some(v) if js_truthy(v) => time_ago(Some(&js_string(v))),
        _ => time_ago(None),
    }
}

/// `array.length` for a JSON array; anything else counts as empty.
pub fn length_of(value: Option<&Value>) -> f64 {
    value.and_then(Value::as_array).map_or(0.0, |items| items.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn template_literals_print_undefined_and_null() {
        assert_eq!(template(None), "undefined");
        assert_eq!(template(Some(&json!(null))), "null");
        assert_eq!(template(Some(&json!("x"))), "x");
        assert_eq!(template(Some(&json!(3))), "3");
        assert_eq!(template(Some(&json!(false))), "false");
        assert_eq!(template(Some(&json!(["a", null]))), "a,");
        assert_eq!(template(Some(&json!({ "a": 1 }))), "[object Object]");
    }

    #[test]
    fn cells_are_empty_for_anything_but_primitives() {
        assert_eq!(cell(Some(&json!("x"))), "x");
        assert_eq!(cell(Some(&json!(1.5))), "1.5");
        assert_eq!(cell(Some(&json!(0))), "0");
        assert_eq!(cell(Some(&json!(false))), "false");
        for empty in [None, Some(json!(null)), Some(json!({ "a": 1 })), Some(json!(["a"]))] {
            assert_eq!(cell(empty.as_ref()), "", "{empty:?}");
        }
    }

    #[test]
    fn nullish_values_take_the_fallback_but_empty_strings_do_not() {
        let dash = || "—".to_string();
        assert_eq!(template_or(None, dash), "—");
        assert_eq!(template_or(Some(&json!(null)), dash), "—");
        assert_eq!(template_or(Some(&json!("")), dash), "");
        assert_eq!(template_or(Some(&json!(0)), dash), "0");
        assert_eq!(template_or(Some(&json!({})), dash), "[object Object]");
        assert_eq!(cell_or(Some(&json!(null)), dash), "—");
        assert_eq!(cell_or(Some(&json!("")), dash), "");
        assert_eq!(cell_or(Some(&json!({})), dash), "");
    }

    #[test]
    fn falsy_dates_are_dashes_and_lengths_count_arrays() {
        for falsy in [None, Some(json!(null)), Some(json!("")), Some(json!(0)), Some(json!(false))] {
            assert_eq!(time_ago_of(falsy.as_ref()), "—", "{falsy:?}");
        }
        assert_eq!(time_ago_of(Some(&json!("not a date"))), "Invalid Date");
        assert_eq!(length_of(Some(&json!([1, 2]))), 2.0);
        assert_eq!(length_of(None), 0.0);
    }
}
