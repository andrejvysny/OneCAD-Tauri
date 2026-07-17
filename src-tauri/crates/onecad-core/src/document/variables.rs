//! Document variables / parameters and the [`Scalar`] dimension type.
//!
//! V1 expressions are a **bare variable name only** (no arithmetic); a real
//! expression engine is deferred (plan "Rust core specifics"). A [`Scalar`] is
//! the unit of every dimensional op parameter (distance, radius, angle …).

use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::ids::VariableId;

/// Unit of a scalar quantity. Minimal for V1 (millimetres only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    /// Millimetre (V1 default and only unit).
    #[default]
    Mm,
}

/// A numeric parameter that may be driven by an expression.
///
/// **Normalizes to the object form on write**: always serializes as
/// `{ "value": <f64> }` (plus `"expr"` when present), never as a bare number.
/// **Deserializes flexibly**: a bare JSON number (`25.0`) becomes
/// `Scalar { value: 25.0, expr: None }`, and the object form is also accepted.
/// Non-finite values (`NaN`/`±Inf`) are rejected (SCHEMA §4).
///
/// File/wire story (kept consistent — SCHEMA §7.3 amended 2026-07-16): SCHEMA
/// §7.3 op examples spell dimensional fields as bare numbers (`"distance": 25.0`)
/// for readability, but a scalar/dimension field **may be either a bare number or
/// a `{value, expr?}` object**, and readers (this core AND the worker) MUST accept
/// both. Because the Rust core normalizes on write, a worker receiving an
/// `ExecutePlan` op authored by the core sees the object form; a hand-authored or
/// legacy payload may carry a bare number.
///
/// V1: `expr`, when set, is a **bare variable name** (looked up in the
/// [`VariableTable`]); arithmetic expressions are deferred.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scalar {
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
}

impl Scalar {
    /// A literal scalar (no expression). Panics on a non-finite value; use
    /// [`Scalar::try_new`] to validate at a boundary.
    #[must_use]
    pub fn new(value: f64) -> Self {
        assert!(value.is_finite(), "Scalar value must be finite");
        Self { value, expr: None }
    }

    /// A literal scalar, rejecting `NaN`/`±Inf` (SCHEMA §4).
    #[must_use]
    pub fn try_new(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self { value, expr: None })
    }

    /// A scalar driven by a bare variable name (V1). `value` is the last
    /// evaluated/cached value.
    #[must_use]
    pub fn with_expr(value: f64, expr: impl Into<String>) -> Self {
        Self {
            value,
            expr: Some(expr.into()),
        }
    }
}

impl<'de> Deserialize<'de> for Scalar {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ScalarVisitor;

        impl<'de> Visitor<'de> for ScalarVisitor {
            type Value = Scalar;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a finite number or a {value, expr?} object")
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Scalar, E> {
                Scalar::try_new(v).ok_or_else(|| de::Error::custom("non-finite Scalar value"))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Scalar, E> {
                Ok(Scalar::new(v as f64))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Scalar, E> {
                Ok(Scalar::new(v as f64))
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Scalar, M::Error> {
                let mut value: Option<f64> = None;
                let mut expr: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "value" => value = Some(map.next_value()?),
                        "expr" => expr = map.next_value()?,
                        // Ignore unknown keys (no deny_unknown_fields).
                        _ => {
                            let _: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let value = value.ok_or_else(|| de::Error::missing_field("value"))?;
                if !value.is_finite() {
                    return Err(de::Error::custom("non-finite Scalar value"));
                }
                Ok(Scalar { value, expr })
            }
        }

        deserializer.deserialize_any(ScalarVisitor)
    }
}

/// A named document variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub id: VariableId,
    pub name: String,
    pub value: Scalar,
    #[serde(default)]
    pub unit: Unit,
}

/// The document's variable table: **ordered** (declaration order is
/// authoritative) with **name-indexed** lookup.
///
/// Serializes transparently as the ordered array of variables.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VariableTable {
    vars: Vec<Variable>,
}

impl VariableTable {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends or replaces (by name) a variable, preserving order.
    pub fn upsert(&mut self, var: Variable) {
        if let Some(existing) = self.vars.iter_mut().find(|v| v.name == var.name) {
            *existing = var;
        } else {
            self.vars.push(var);
        }
    }

    /// Looks up a variable by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Variable> {
        self.vars.iter().find(|v| v.name == name)
    }

    /// Iterates variables in declaration order.
    pub fn iter(&self) -> std::slice::Iter<'_, Variable> {
        self.vars.iter()
    }

    /// Number of variables.
    #[must_use]
    pub fn len(&self) -> usize {
        self.vars.len()
    }

    /// True iff there are no variables.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_parses_bare_number_and_object() {
        let a: Scalar = serde_json::from_str("25.0").unwrap();
        assert_eq!(a, Scalar::new(25.0));
        let b: Scalar = serde_json::from_str("7").unwrap();
        assert_eq!(b.value, 7.0);
        let c: Scalar = serde_json::from_str(r#"{"value": 3.0, "expr": "width"}"#).unwrap();
        assert_eq!(c, Scalar::with_expr(3.0, "width"));
        // Unknown keys inside a Scalar object are ignored (no deny_unknown_fields).
        let d: Scalar = serde_json::from_str(r#"{"value": 1.0, "junk": true}"#).unwrap();
        assert_eq!(d.value, 1.0);
    }

    #[test]
    fn scalar_serializes_as_object_and_skips_none_expr() {
        assert_eq!(
            serde_json::to_string(&Scalar::new(2.5)).unwrap(),
            r#"{"value":2.5}"#
        );
        assert_eq!(
            serde_json::to_string(&Scalar::with_expr(2.5, "w")).unwrap(),
            r#"{"value":2.5,"expr":"w"}"#
        );
    }

    #[test]
    fn scalar_rejects_non_finite() {
        assert!(Scalar::try_new(f64::NAN).is_none());
        assert!(serde_json::from_str::<Scalar>("1e999").is_err());
    }

    #[test]
    fn variable_table_is_ordered_with_name_lookup() {
        let mut t = VariableTable::new();
        let v = |name: &str, val: f64| Variable {
            id: VariableId::new(),
            name: name.to_string(),
            value: Scalar::new(val),
            unit: Unit::Mm,
        };
        t.upsert(v("width", 10.0));
        t.upsert(v("height", 20.0));
        t.upsert(v("width", 15.0)); // replace, keep order
        assert_eq!(t.len(), 2);
        assert_eq!(t.get("width").unwrap().value.value, 15.0);
        let names: Vec<_> = t.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["width", "height"]);
    }
}
