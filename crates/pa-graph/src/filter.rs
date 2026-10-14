//! Payload-filter expressions compiled to parameterized SQL `WHERE` fragments.
//!
//! A [`Filter`] is a small `AND`/`OR`/`NOT` tree over node/edge properties
//! (`props_json`). [`Filter::compile`] turns it into a SQL predicate plus its
//! bound parameters, so the value is never interpolated into the SQL text —
//! the compiled fragment is injection-safe by construction.
//!
//! ```text
//! Filter::and(vec![
//!     Filter::eq("label", Value::from("Function")),
//!     Filter::gte("complexity", Value::from(5)),
//! ])
//! ```
//!
//! Properties are read from the JSON column with `json_extract`. `label` and
//! `name` are treated as first-class columns so callers can filter on them
//! without knowing the storage layout.

use serde_json::Value;

use crate::error::{GraphError, Result};

/// A comparison operator over a property value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `=`
    Eq,
    /// `!=`
    Ne,
    /// `>`
    Gt,
    /// `>=`
    Gte,
    /// `<`
    Lt,
    /// `<=`
    Lte,
    /// Substring match on the JSON text.
    Contains,
    /// Exact JSON array membership.
    In,
}

/// A single leaf comparison: `field OP value`.
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    /// Property name, or one of the column aliases `label`/`name`.
    pub field: String,
    /// Comparison operator.
    pub op: Op,
    /// Right-hand side. For [`Op::In`] this must be a JSON array.
    pub value: Value,
}

/// A boolean filter expression over node properties.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    /// A leaf comparison.
    Cond(Condition),
    /// Every child must hold.
    And(Vec<Self>),
    /// At least one child must hold.
    Or(Vec<Self>),
    /// The child must not hold.
    Not(Box<Self>),
}

impl Filter {
    /// Build a leaf condition.
    #[must_use]
    pub fn cond(field: impl Into<String>, op: Op, value: Value) -> Self {
        Self::Cond(Condition {
            field: field.into(),
            op,
            value,
        })
    }

    /// `field == value`.
    #[must_use]
    pub fn eq(field: impl Into<String>, value: Value) -> Self {
        Self::cond(field, Op::Eq, value)
    }

    /// `field != value`.
    #[must_use]
    pub fn ne(field: impl Into<String>, value: Value) -> Self {
        Self::cond(field, Op::Ne, value)
    }

    /// `field >= value`.
    #[must_use]
    pub fn gte(field: impl Into<String>, value: Value) -> Self {
        Self::cond(field, Op::Gte, value)
    }

    /// `field > value`.
    #[must_use]
    pub fn gt(field: impl Into<String>, value: Value) -> Self {
        Self::cond(field, Op::Gt, value)
    }

    /// `field <= value`.
    #[must_use]
    pub fn lte(field: impl Into<String>, value: Value) -> Self {
        Self::cond(field, Op::Lte, value)
    }

    /// `field < value`.
    #[must_use]
    pub fn lt(field: impl Into<String>, value: Value) -> Self {
        Self::cond(field, Op::Lt, value)
    }

    /// Substring match on the field's text.
    #[must_use]
    pub fn contains(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self::cond(field, Op::Contains, Value::String(value.into()))
    }

    /// Membership in a JSON array.
    #[must_use]
    pub fn is_in(field: impl Into<String>, values: Vec<Value>) -> Self {
        Self::cond(field, Op::In, Value::Array(values))
    }

    /// Conjunction of `parts`.
    #[must_use]
    pub fn and(parts: Vec<Self>) -> Self {
        Self::And(parts)
    }

    /// Disjunction of `parts`.
    #[must_use]
    pub fn or(parts: Vec<Self>) -> Self {
        Self::Or(parts)
    }

    /// Negation of `inner`.
    #[must_use]
    pub fn negate(inner: Self) -> Self {
        Self::Not(Box::new(inner))
    }

    /// Compile to a SQL predicate and its ordered bound parameters.
    ///
    /// The returned string references positional parameters `?1..?n` *relative
    /// to the fragment*, so callers prepend their own parameters and offset
    /// these accordingly (see [`crate::search`], which appends the fragment
    /// after its own `?1` project argument and renumbers via `?N`).
    ///
    /// # Errors
    /// Returns [`GraphError::InvalidFilter`] for an empty `AND`/`OR`, an
    /// `IN` with a non-array value, or an unknown operator/value combination.
    pub fn compile(&self) -> Result<(String, Vec<Value>)> {
        self.compile_prefixed("")
    }

    /// Like [`Self::compile`] but qualifies column aliases with `prefix`
    /// (e.g. `"n."`), which is required when the filter runs against a join
    /// where `name`/`label` exist in more than one table.
    ///
    /// # Errors
    /// Same validation as [`Self::compile`].
    pub fn compile_prefixed(&self, prefix: &str) -> Result<(String, Vec<Value>)> {
        let mut params = Vec::new();
        let sql = self.compile_into(prefix, &mut params)?;
        Ok((sql, params))
    }

    fn compile_into(&self, prefix: &str, params: &mut Vec<Value>) -> Result<String> {
        match self {
            Self::Cond(c) => compile_cond(c, prefix, params),
            Self::And(parts) => {
                if parts.is_empty() {
                    return Err(GraphError::InvalidFilter("AND with no children".into()));
                }
                let clauses = parts
                    .iter()
                    .map(|p| p.compile_into(prefix, params))
                    .collect::<Result<Vec<_>>>()?;
                Ok(format!("({})", clauses.join(" AND ")))
            }
            Self::Or(parts) => {
                if parts.is_empty() {
                    return Err(GraphError::InvalidFilter("OR with no children".into()));
                }
                let clauses = parts
                    .iter()
                    .map(|p| p.compile_into(prefix, params))
                    .collect::<Result<Vec<_>>>()?;
                Ok(format!("({})", clauses.join(" OR ")))
            }
            Self::Not(inner) => Ok(format!("(NOT {})", inner.compile_into(prefix, params)?)),
        }
    }

    /// Number of bound parameters the compiled fragment expects.
    ///
    /// # Errors
    /// Propagates [`Self::compile`]'s validation.
    pub fn param_count(&self) -> Result<usize> {
        Ok(self.compile()?.1.len())
    }
}

/// Shift every `?N` placeholder in `sql` by `offset`, returning the rewritten
/// fragment. Used to splice a compiled filter after a query's own parameters.
#[must_use]
pub fn renumber_placeholders(sql: &str, offset: usize) -> String {
    if offset == 0 {
        return sql.to_string();
    }
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + 8);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'?' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            let n: usize = sql[start..end].parse().unwrap_or(0);
            out.push('?');
            out.push_str(&(n + offset).to_string());
            i = end;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Render the SQL expression selecting a property's JSON value.
///
/// `label` and `name` are promoted to their real columns; every other field is
/// read from `props_json`.
fn field_expr(field: &str, prefix: &str) -> String {
    match field {
        "label" => format!("{prefix}label"),
        "name" => format!("{prefix}name"),
        "qualified_name" => format!("{prefix}qualified_name"),
        "file_path" => format!("{prefix}file_path"),
        "start_line" => format!("{prefix}start_line"),
        "end_line" => format!("{prefix}end_line"),
        other => format!(
            "json_extract({prefix}props_json, '$.{}')",
            escape_json_path(other)
        ),
    }
}

/// Escape a property name for embedding inside a JSON path literal.
///
/// Only `'` and `"` need care inside a single-quoted SQLite string; the JSON
/// path itself is `$.name`, and a property containing a quote is rejected
/// upstream by the strict character check.
fn escape_json_path(field: &str) -> String {
    field.replace('\'', "''")
}

fn compile_cond(c: &Condition, prefix: &str, params: &mut Vec<Value>) -> Result<String> {
    let expr = field_expr(&c.field, prefix);
    match c.op {
        Op::Eq => {
            params.push(c.value.clone());
            Ok(format!("{expr} = {}", placeholder(params.len())))
        }
        Op::Ne => {
            params.push(c.value.clone());
            Ok(format!("{expr} != {}", placeholder(params.len())))
        }
        Op::Gt | Op::Gte | Op::Lt | Op::Lte => {
            let op = match c.op {
                Op::Gt => ">",
                Op::Gte => ">=",
                Op::Lt => "<",
                Op::Lte => "<=",
                _ => unreachable!("guarded by the match arm"),
            };
            let numeric = json_scalar(&c.value).ok_or_else(|| {
                GraphError::InvalidFilter(format!("{:?} needs a numeric value", c.op))
            })?;
            params.push(Value::from(numeric));
            Ok(format!(
                "CAST({expr} AS REAL) {op} {}",
                placeholder(params.len())
            ))
        }
        Op::Contains => {
            let needle = c
                .value
                .as_str()
                .ok_or_else(|| GraphError::InvalidFilter("contains needs a string value".into()))?;
            params.push(Value::String(format!("%{needle}%")));
            Ok(format!(
                "CAST({expr} AS TEXT) LIKE {} ESCAPE '\\'",
                placeholder(params.len())
            ))
        }
        Op::In => {
            let items = c
                .value
                .as_array()
                .ok_or_else(|| GraphError::InvalidFilter("in needs an array value".into()))?;
            if items.is_empty() {
                return Ok("0".to_string());
            }
            let mut holes = Vec::with_capacity(items.len());
            for item in items {
                params.push(item.clone());
                holes.push(placeholder(params.len()));
            }
            Ok(format!("{expr} IN ({})", holes.join(", ")))
        }
    }
}

fn placeholder(n: usize) -> String {
    format!("?{n}")
}

/// Extract a numeric value from a JSON scalar (bool → 0/1).
fn json_scalar(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn eq_compiles_to_parameterized_predicate() {
        let f = Filter::eq("label", json!("Function"));
        let (sql, params) = f.compile().expect("compile");
        assert_eq!(sql, "label = ?1");
        assert_eq!(params, vec![json!("Function")]);
    }

    #[test]
    fn prop_field_uses_json_extract() {
        let f = Filter::gte("complexity", json!(5));
        let (sql, params) = f.compile().expect("compile");
        assert_eq!(
            sql,
            "CAST(json_extract(props_json, '$.complexity') AS REAL) >= ?1"
        );
        assert_eq!(params, vec![json!(5.0)]);
    }

    #[test]
    fn and_or_not_nest_and_number_params() {
        let f = Filter::and(vec![
            Filter::eq("label", json!("Method")),
            Filter::or(vec![
                Filter::gt("score", json!(0.5)),
                Filter::negate(Filter::contains("name", "test")),
            ]),
        ]);
        let (sql, params) = f.compile().expect("compile");
        assert_eq!(
            sql,
            "(label = ?1 AND (CAST(json_extract(props_json, '$.score') AS REAL) > ?2 \
             OR (NOT CAST(name AS TEXT) LIKE ?3 ESCAPE '\\')))"
        );
        assert_eq!(params.len(), 3);
        assert_eq!(params[2], json!("%test%"));
    }

    #[test]
    fn in_expands_holes() {
        let f = Filter::is_in("label", vec![json!("Function"), json!("Method")]);
        let (sql, params) = f.compile().expect("compile");
        assert_eq!(sql, "label IN (?1, ?2)");
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn empty_in_matches_nothing() {
        let f = Filter::is_in("label", vec![]);
        let (sql, _) = f.compile().expect("compile");
        assert_eq!(sql, "0");
    }

    #[test]
    fn empty_and_is_rejected() {
        assert!(Filter::and(vec![]).compile().is_err());
        assert!(Filter::or(vec![]).compile().is_err());
    }

    #[test]
    fn range_needs_numeric() {
        let f = Filter::gt("name", json!("abc"));
        assert!(f.compile().is_err());
    }

    #[test]
    fn bool_compares_numerically() {
        let (sql, params) = Filter::eq("exported", json!(true))
            .compile()
            .expect("compile");
        assert_eq!(sql, "json_extract(props_json, '$.exported') = ?1");
        assert_eq!(params, vec![json!(true)]);
    }

    #[test]
    fn renumber_shifts_all_placeholders() {
        assert_eq!(
            renumber_placeholders("a = ?1 AND b > ?2", 3),
            "a = ?4 AND b > ?5"
        );
        assert_eq!(renumber_placeholders("x IN (?1, ?10)", 1), "x IN (?2, ?11)");
        assert_eq!(renumber_placeholders("no params", 5), "no params");
    }

    #[test]
    fn param_count_matches() {
        let f = Filter::or(vec![Filter::eq("a", json!(1)), Filter::eq("b", json!(2))]);
        assert_eq!(f.param_count().expect("count"), 2);
    }
}
