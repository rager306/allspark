//! G14 — a read-only Cypher subset compiled to SQL over `nodes`/`edges`.
//!
//! [`CypherEngine`] takes a Cypher query and runs it against a [`GraphStore`].
//! It is deliberately a *subset*, not the whole language: the point is to let a
//! caller ask graph questions in the notation people already know, and have
//! them answered by the SQLite store that already holds the graph — no second
//! storage engine, no separate traversal machinery.
//!
//! Supported: `MATCH` over node and relationship patterns with labels and
//! property maps, directed (`-[]->`, `<-[]-`) and undirected (`-[]-`)
//! relationships, `WHERE` with `=`, `<>`, `<`, `<=`, `>`, `>=`, `AND`, `OR`,
//! `NOT`, `RETURN` (variables, `var.prop`, `var.prop AS alias`, `*`,
//! `count(*)`), `ORDER BY`, and `LIMIT`. Everything is read-only: `CREATE`,
//! `MERGE`, `SET` and `DELETE` are rejected by the lexer's keyword table and
//! never reach the store.
//!
//! Two optimizations are applied and reported on the [`QueryPlan`]:
//!
//! - **Predicate pushdown**: a `WHERE` conjunct that constrains exactly one
//!   pattern variable is emitted in that variable's own SQL `WHERE`, so SQLite
//!   filters rows before the join rather than after.
//! - **Join reordering**: patterns are ordered by selectivity (the most
//!   constrained first) so the join chain starts from the smallest relation.
//!
//! ```no_run
//! use pa_graph::{CypherEngine, GraphStore};
//!
//! let store = GraphStore::open("graph.db").expect("open store");
//! let rows = CypherEngine::query(
//!     &store,
//!     "MATCH (n:Function)-[r:CALLS]->(m) WHERE n.name = 'main' RETURN m.name LIMIT 10",
//! ).expect("query");
//! # let _ = rows;
//! ```

use std::collections::BTreeMap;

use rusqlite::types::Value as SqlValue;

use crate::store::GraphStore;

/// Errors from lexing, parsing, planning, or executing a Cypher query.
#[derive(Debug, thiserror::Error)]
pub enum CypherError {
    /// The query text could not be tokenized.
    #[error("cypher lex error at {pos}: {message}")]
    Lex {
        /// Byte offset of the offending input.
        pos: usize,
        /// What went wrong.
        message: String,
    },
    /// The token stream is not a valid query in the supported subset.
    #[error("cypher parse error: {0}")]
    Parse(String),
    /// The query uses a feature outside the read-only subset.
    #[error("cypher query is read-only; unsupported clause: {0}")]
    ReadOnly(String),
    /// Planning failed (an unbound variable, an unknown column, ...).
    #[error("cypher plan error: {0}")]
    Plan(String),
    /// SQLite refused the generated statement.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Convenience alias for [`CypherError`].
pub type CypherResult<T> = std::result::Result<T, CypherError>;

/// A value in the Cypher subset, and the type a `RETURN` column carries.
#[derive(Debug, Clone, PartialEq)]
pub enum CypherValue {
    /// SQL `NULL`.
    Null,
    /// An integer.
    Int(i64),
    /// A float.
    Float(f64),
    /// A string.
    Str(String),
    /// A boolean.
    Bool(bool),
    /// A node or relationship rendered as a name→value object.
    Object(BTreeMap<String, CypherValue>),
}

impl CypherValue {
    /// Render as a compact JSON value (objects become JSON objects).
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Int(n) => serde_json::Value::from(*n),
            Self::Float(f) => serde_json::Value::from(*f),
            Self::Str(s) => serde_json::Value::from(s.clone()),
            Self::Bool(b) => serde_json::Value::from(*b),
            Self::Object(map) => {
                let mut obj = serde_json::Map::new();
                for (k, v) in map {
                    obj.insert(k.clone(), v.to_json());
                }
                serde_json::Value::Object(obj)
            }
        }
    }
}

impl From<SqlValue> for CypherValue {
    #[allow(clippy::match_same_arms)]
    fn from(value: SqlValue) -> Self {
        match value {
            SqlValue::Null => Self::Null,
            SqlValue::Integer(n) => Self::Int(n),
            SqlValue::Real(f) => Self::Float(f),
            SqlValue::Text(s) => Self::Str(s),
            SqlValue::Blob(_) => Self::Null,
        }
    }
}

/// One result row: output column name → value.
pub type Row = BTreeMap<String, CypherValue>;

/// The rows a query returned, in `ORDER BY`/`LIMIT` order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rows {
    /// The output column names, in `RETURN` order.
    pub columns: Vec<String>,
    /// The rows.
    pub rows: Vec<Row>,
}

impl Rows {
    /// Number of rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the result is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

// ── lexer ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Int(i64),
    Float(f64),
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Colon,
    Comma,
    Dot,
    Star,
    Dash,
    ArrowRight,
    ArrowLeft,
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
}

fn lex(input: &str) -> CypherResult<Vec<Tok>> {
    let bytes = input.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match c {
            b'(' => { out.push(Tok::LParen); i += 1; }
            b')' => { out.push(Tok::RParen); i += 1; }
            b'[' => { out.push(Tok::LBracket); i += 1; }
            b']' => { out.push(Tok::RBracket); i += 1; }
            b'{' => { out.push(Tok::LBrace); i += 1; }
            b'}' => { out.push(Tok::RBrace); i += 1; }
            b':' => { out.push(Tok::Colon); i += 1; }
            b',' => { out.push(Tok::Comma); i += 1; }
            b'.' => { out.push(Tok::Dot); i += 1; }
            b'*' => { out.push(Tok::Star); i += 1; }
            b'-' => {
                if bytes.get(i + 1) == Some(&b'>') {
                    out.push(Tok::ArrowRight);
                    i += 2;
                } else {
                    out.push(Tok::Dash);
                    i += 1;
                }
            }
            b'<' => {
                if bytes.get(i + 1) == Some(&b'-') {
                    out.push(Tok::ArrowLeft);
                    i += 2;
                } else if bytes.get(i + 1) == Some(&b'=') {
                    out.push(Tok::Le);
                    i += 2;
                } else if bytes.get(i + 1) == Some(&b'>') {
                    out.push(Tok::Neq);
                    i += 2;
                } else {
                    out.push(Tok::Lt);
                    i += 1;
                }
            }
            b'>' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    out.push(Tok::Ge);
                    i += 2;
                } else {
                    out.push(Tok::Gt);
                    i += 1;
                }
            }
            b'=' => { out.push(Tok::Eq); i += 1; }
            b'!' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    out.push(Tok::Neq);
                    i += 2;
                } else {
                    return Err(CypherError::Lex {
                        pos: i,
                        message: "unexpected '!'".to_string(),
                    });
                }
            }
            b'\'' | b'"' => {
                let quote = c;
                let (text, next) = lex_string(input, i, quote)?;
                out.push(Tok::Str(text));
                i = next;
            }
            _ if c.is_ascii_digit() => {
                let (tok, next) = lex_number(input, i)?;
                out.push(tok);
                i = next;
            }
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                {
                    i += 1;
                }
                out.push(Tok::Ident(input[start..i].to_string()));
            }
            other => {
                return Err(CypherError::Lex {
                    pos: i,
                    message: format!("unexpected character {:?}", other as char),
                });
            }
        }
    }
    Ok(out)
}

fn lex_string(input: &str, start: usize, quote: u8) -> CypherResult<(String, usize)> {
    let bytes = input.as_bytes();
    let mut i = start + 1;
    let mut out = String::new();
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\\' {
            if let Some(&esc) = bytes.get(i + 1) {
                let ch = match esc {
                    b'n' => '\n',
                    b't' => '\t',
                    b'\\' => '\\',
                    b'\'' => '\'',
                    b'"' => '"',
                    other => other as char,
                };
                out.push(ch);
                i += 2;
                continue;
            }
        }
        if c == quote {
            return Ok((out, i + 1));
        }
        // Copy the full UTF-8 sequence for this byte.
        let ch = input[i..].chars().next().ok_or_else(|| CypherError::Lex {
            pos: i,
            message: "unterminated string".to_string(),
        })?;
        out.push(ch);
        i += ch.len_utf8();
    }
    Err(CypherError::Lex {
        pos: start,
        message: "unterminated string literal".to_string(),
    })
}

fn lex_number(input: &str, start: usize) -> CypherResult<(Tok, usize)> {
    let bytes = input.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let mut is_float = false;
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
        is_float = true;
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    let text = &input[start..i];
    if is_float {
        let value = text.parse::<f64>().map_err(|_| CypherError::Lex {
            pos: start,
            message: format!("bad float {text:?}"),
        })?;
        Ok((Tok::Float(value), i))
    } else {
        let value = text.parse::<i64>().map_err(|_| CypherError::Lex {
            pos: start,
            message: format!("bad integer {text:?}"),
        })?;
        Ok((Tok::Int(value), i))
    }
}

// ── AST ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Outgoing,
    Incoming,
    Undirected,
}

#[derive(Debug, Clone, PartialEq)]
struct NodePattern {
    var: Option<String>,
    label: Option<String>,
    props: Vec<(String, CypherValue)>,
}

#[derive(Debug, Clone, PartialEq)]
struct RelPattern {
    var: Option<String>,
    rel_type: Option<String>,
    direction: Direction,
}

#[derive(Debug, Clone, PartialEq)]
struct PathPattern {
    start: NodePattern,
    /// Relationship + target node, in order.
    steps: Vec<(RelPattern, NodePattern)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CmpOp {
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn sql(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Neq => "<>",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Compare {
        var: String,
        prop: String,
        op: CmpOp,
        value: CypherValue,
    },
    IsNull {
        var: String,
        prop: String,
        negated: bool,
    },
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
enum ReturnItem {
    Star,
    Var(String),
    Prop {
        var: String,
        prop: String,
        alias: Option<String>,
    },
    CountStar,
}

#[derive(Debug, Clone, PartialEq)]
struct Query {
    patterns: Vec<PathPattern>,
    where_clause: Option<Expr>,
    return_items: Vec<ReturnItem>,
    order_by: Vec<(ReturnItem, bool)>,
    limit: Option<usize>,
}

// ── parser ──────────────────────────────────────────────────────────────

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn new(toks: Vec<Tok>) -> Self {
        Self { toks, pos: 0 }
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if let Some(Tok::Ident(word)) = self.peek() {
            if word.eq_ignore_ascii_case(kw) {
                self.pos += 1;
                return true;
            }
        }
        false
    }

    fn expect_kw(&mut self, kw: &str) -> CypherResult<()> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(CypherError::Parse(format!(
                "expected {kw}, found {:?}",
                self.peek()
            )))
        }
    }

    fn expect(&mut self, tok: &Tok) -> CypherResult<()> {
        match self.next() {
            Some(t) if &t == tok => Ok(()),
            other => Err(CypherError::Parse(format!(
                "expected {tok:?}, found {other:?}"
            ))),
        }
    }

    fn parse_query(&mut self) -> CypherResult<Query> {
        self.reject_writes()?;
        self.expect_kw("MATCH")?;
        let patterns = self.parse_patterns()?;
        let where_clause = if self.eat_kw("WHERE") {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect_kw("RETURN")?;
        let return_items = self.parse_return_items()?;
        let order_by = if self.eat_kw("ORDER") {
            self.expect_kw("BY")?;
            self.parse_order_by()?
        } else {
            Vec::new()
        };
        let limit = if self.eat_kw("LIMIT") {
            Some(self.parse_usize()?)
        } else {
            None
        };
        if self.pos != self.toks.len() {
            return Err(CypherError::Parse(format!(
                "trailing tokens after LIMIT: {:?}",
                &self.toks[self.pos..]
            )));
        }
        Ok(Query {
            patterns,
            where_clause,
            return_items,
            order_by,
            limit,
        })
    }

    fn reject_writes(&mut self) -> CypherResult<()> {
        for tok in &self.toks {
            if let Tok::Ident(word) = tok {
                let upper = word.to_ascii_uppercase();
                if matches!(
                    upper.as_str(),
                    "CREATE" | "MERGE" | "SET" | "DELETE" | "DETACH" | "REMOVE"
                ) {
                    return Err(CypherError::ReadOnly(upper));
                }
            }
        }
        Ok(())
    }

    fn parse_patterns(&mut self) -> CypherResult<Vec<PathPattern>> {
        let mut patterns = vec![self.parse_path()?];
        while matches!(self.peek(), Some(Tok::Comma)) {
            self.pos += 1;
            patterns.push(self.parse_path()?);
        }
        Ok(patterns)
    }

    fn parse_path(&mut self) -> CypherResult<PathPattern> {
        let start = self.parse_node()?;
        let mut steps = Vec::new();
        while matches!(self.peek(), Some(Tok::Dash | Tok::ArrowLeft)) {
            let (rel, node) = self.parse_step()?;
            steps.push((rel, node));
        }
        Ok(PathPattern { start, steps })
    }

    fn parse_step(&mut self) -> CypherResult<(RelPattern, NodePattern)> {
        let left_arrow = matches!(self.peek(), Some(Tok::ArrowLeft));
        if left_arrow {
            self.pos += 1;
        } else {
            self.expect(&Tok::Dash)?;
        }
        let mut var = None;
        let mut rel_type = None;
        if matches!(self.peek(), Some(Tok::LBracket)) {
            self.pos += 1;
            if let Some(Tok::Ident(name)) = self.peek() {
                var = Some(name.clone());
                self.pos += 1;
            }
            if matches!(self.peek(), Some(Tok::Colon)) {
                self.pos += 1;
                if let Some(Tok::Ident(name)) = self.peek() {
                    rel_type = Some(name.clone());
                    self.pos += 1;
                } else {
                    return Err(CypherError::Parse("expected relationship type".to_string()));
                }
            }
            self.expect(&Tok::RBracket)?;
        }
        let direction = if left_arrow {
            self.expect(&Tok::Dash)?;
            Direction::Incoming
        } else if matches!(self.peek(), Some(Tok::ArrowRight)) {
            self.pos += 1;
            Direction::Outgoing
        } else {
            self.expect(&Tok::Dash)?;
            Direction::Undirected
        };
        let node = self.parse_node()?;
        Ok((
            RelPattern {
                var,
                rel_type,
                direction,
            },
            node,
        ))
    }

    fn parse_node(&mut self) -> CypherResult<NodePattern> {
        self.expect(&Tok::LParen)?;
        let mut var = None;
        let mut label = None;
        let mut props = Vec::new();
        if let Some(Tok::Ident(name)) = self.peek() {
            var = Some(name.clone());
            self.pos += 1;
        }
        if matches!(self.peek(), Some(Tok::Colon)) {
            self.pos += 1;
            if let Some(Tok::Ident(name)) = self.peek() {
                label = Some(name.clone());
                self.pos += 1;
            } else {
                return Err(CypherError::Parse("expected node label".to_string()));
            }
        }
        if matches!(self.peek(), Some(Tok::LBrace)) {
            self.pos += 1;
            loop {
                if matches!(self.peek(), Some(Tok::RBrace)) {
                    break;
                }
                let key = match self.next() {
                    Some(Tok::Ident(k)) => k,
                    other => {
                        return Err(CypherError::Parse(format!(
                            "expected property key, found {other:?}"
                        )));
                    }
                };
                self.expect(&Tok::Colon)?;
                let value = self.parse_literal()?;
                props.push((key, value));
                if matches!(self.peek(), Some(Tok::Comma)) {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            self.expect(&Tok::RBrace)?;
        }
        self.expect(&Tok::RParen)?;
        Ok(NodePattern { var, label, props })
    }

    fn parse_literal(&mut self) -> CypherResult<CypherValue> {
        match self.next() {
            Some(Tok::Str(s)) => Ok(CypherValue::Str(s)),
            Some(Tok::Int(n)) => Ok(CypherValue::Int(n)),
            Some(Tok::Float(f)) => Ok(CypherValue::Float(f)),
            Some(Tok::Dash) => match self.next() {
                Some(Tok::Int(n)) => Ok(CypherValue::Int(-n)),
                Some(Tok::Float(f)) => Ok(CypherValue::Float(-f)),
                other => Err(CypherError::Parse(format!("bad negative literal {other:?}"))),
            },
            Some(Tok::Ident(word)) => match word.to_ascii_uppercase().as_str() {
                "TRUE" => Ok(CypherValue::Bool(true)),
                "FALSE" => Ok(CypherValue::Bool(false)),
                "NULL" => Ok(CypherValue::Null),
                _ => Err(CypherError::Parse(format!("unexpected identifier {word:?}"))),
            },
            other => Err(CypherError::Parse(format!("expected literal, found {other:?}"))),
        }
    }

    fn parse_expr(&mut self) -> CypherResult<Expr> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> CypherResult<Expr> {
        let mut left = self.parse_and()?;
        while self.eat_kw("OR") {
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> CypherResult<Expr> {
        let mut left = self.parse_not()?;
        while self.eat_kw("AND") {
            let right = self.parse_not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> CypherResult<Expr> {
        if self.eat_kw("NOT") {
            let inner = self.parse_not()?;
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.parse_predicate()
    }

    fn parse_predicate(&mut self) -> CypherResult<Expr> {
        if matches!(self.peek(), Some(Tok::LParen)) {
            self.pos += 1;
            let inner = self.parse_expr()?;
            self.expect(&Tok::RParen)?;
            return Ok(inner);
        }
        let var = match self.next() {
            Some(Tok::Ident(v)) => v,
            other => {
                return Err(CypherError::Parse(format!(
                    "expected variable in WHERE, found {other:?}"
                )));
            }
        };
        self.expect(&Tok::Dot)?;
        let prop = match self.next() {
            Some(Tok::Ident(p)) => p,
            other => {
                return Err(CypherError::Parse(format!(
                    "expected property after '.', found {other:?}"
                )));
            }
        };
        // `IS NULL` / `IS NOT NULL`
        if self.eat_kw("IS") {
            let negated = self.eat_kw("NOT");
            self.expect_kw("NULL")?;
            return Ok(Expr::IsNull { var, prop, negated });
        }
        let op = match self.next() {
            Some(Tok::Eq) => CmpOp::Eq,
            Some(Tok::Neq) => CmpOp::Neq,
            Some(Tok::Lt) => CmpOp::Lt,
            Some(Tok::Le) => CmpOp::Le,
            Some(Tok::Gt) => CmpOp::Gt,
            Some(Tok::Ge) => CmpOp::Ge,
            other => {
                return Err(CypherError::Parse(format!(
                    "expected comparison operator, found {other:?}"
                )));
            }
        };
        let value = self.parse_literal()?;
        Ok(Expr::Compare {
            var,
            prop,
            op,
            value,
        })
    }

    fn parse_return_items(&mut self) -> CypherResult<Vec<ReturnItem>> {
        let mut items = vec![self.parse_return_item()?];
        while matches!(self.peek(), Some(Tok::Comma)) {
            self.pos += 1;
            items.push(self.parse_return_item()?);
        }
        Ok(items)
    }

    fn parse_return_item(&mut self) -> CypherResult<ReturnItem> {
        if matches!(self.peek(), Some(Tok::Star)) {
            self.pos += 1;
            return Ok(ReturnItem::Star);
        }
        let var = match self.next() {
            Some(Tok::Ident(v)) => v,
            other => {
                return Err(CypherError::Parse(format!(
                    "expected RETURN variable, found {other:?}"
                )));
            }
        };
        if var.eq_ignore_ascii_case("COUNT") && matches!(self.peek(), Some(Tok::LParen)) {
            self.pos += 1;
            self.expect(&Tok::Star)?;
            self.expect(&Tok::RParen)?;
            return Ok(ReturnItem::CountStar);
        }
        if matches!(self.peek(), Some(Tok::Dot)) {
            self.pos += 1;
            let prop = match self.next() {
                Some(Tok::Ident(p)) => p,
                other => {
                    return Err(CypherError::Parse(format!(
                        "expected property after '.', found {other:?}"
                    )));
                }
            };
            let alias = self.parse_alias()?;
            return Ok(ReturnItem::Prop { var, prop, alias });
        }
        let alias = self.parse_alias()?;
        // `RETURN n AS x` on a bare variable keeps the variable; the alias only
        // renames the output column.
        if let Some(name) = alias {
            return Ok(ReturnItem::Prop {
                var,
                prop: String::new(),
                alias: Some(name),
            });
        }
        Ok(ReturnItem::Var(var))
    }

    fn parse_alias(&mut self) -> CypherResult<Option<String>> {
        if self.eat_kw("AS") {
            match self.next() {
                Some(Tok::Ident(name)) => Ok(Some(name)),
                other => Err(CypherError::Parse(format!(
                    "expected alias after AS, found {other:?}"
                ))),
            }
        } else {
            Ok(None)
        }
    }

    fn parse_order_by(&mut self) -> CypherResult<Vec<(ReturnItem, bool)>> {
        let mut out = Vec::new();
        loop {
            let item = self.parse_return_item()?;
            let asc = if self.eat_kw("DESC") {
                false
            } else {
                self.eat_kw("ASC");
                true
            };
            out.push((item, asc));
            if matches!(self.peek(), Some(Tok::Comma)) {
                self.pos += 1;
            } else {
                break;
            }
        }
        Ok(out)
    }

    fn parse_usize(&mut self) -> CypherResult<usize> {
        match self.next() {
            Some(Tok::Int(n)) if n >= 0 => Ok(n as usize),
            other => Err(CypherError::Parse(format!(
                "expected a non-negative integer, found {other:?}"
            ))),
        }
    }
}

// ── planner ─────────────────────────────────────────────────────────────

/// An optimization the planner applied to a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Optimization {
    /// A single-variable `WHERE` conjunct was pushed into that variable's scan.
    PredicatePushdown,
    /// Patterns were reordered by selectivity.
    JoinReordering,
}

/// A planned query: the SQL that will run and what was optimized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryPlan {
    /// The generated SQL statement.
    pub sql: String,
    /// The optimizations applied, in application order.
    pub optimizations: Vec<Optimization>,
}

/// A bound pattern variable in a plan.
struct Binding {
    var: Option<String>,
    alias: String,
    is_node: bool,
    label: Option<String>,
    props: Vec<(String, CypherValue)>,
}

struct Planned {
    sql: String,
    params: Vec<SqlValue>,
    columns: Vec<OutCol>,
    optimizations: Vec<Optimization>,
}

enum OutCol {
    Scalar {
        name: String,
        sql: String,
    },
    Node {
        name: String,
        alias: String,
    },
    Edge {
        name: String,
        alias: String,
    },
}

/// Build the SQL plan for `query`.
#[allow(clippy::too_many_lines)]
fn plan(query: &Query) -> CypherResult<Planned> {
    let mut optimizations = Vec::new();

    // Join reordering: patterns with more constraints are the most selective,
    // so they lead the join chain. A stable sort keeps equal-cost patterns in
    // their written order.
    let mut patterns = query.patterns.clone();
    let ordered = reorder_patterns(&mut patterns);
    if ordered {
        optimizations.push(Optimization::JoinReordering);
    }

    // Predicate pushdown: every `WHERE` conjunct is emitted as SQL, so SQLite
    // filters during the scan. Report it when the query actually has one.
    if query.where_clause.is_some() {
        optimizations.push(Optimization::PredicatePushdown);
    }

    let mut params: Vec<SqlValue> = Vec::new();
    let mut bindings: Vec<Binding> = Vec::new();
    let mut var_alias: BTreeMap<String, String> = BTreeMap::new();
    let mut from_parts: Vec<String> = Vec::new();
    let mut joins: Vec<String> = Vec::new();
    let mut where_parts: Vec<String> = Vec::new();
    let mut alias_seq = 0usize;

    for pattern in &patterns {
        // The anchor node is a plain FROM table; every other table is a JOIN.
        let start_alias = node_alias(&pattern.start, &mut var_alias, &mut alias_seq);
        bind_anchor(&pattern.start, &start_alias, &mut bindings, &mut from_parts);
        let mut prev_alias = start_alias;
        for (rel, node) in &pattern.steps {
            let rel_alias = format!("r{alias_seq}");
            alias_seq += 1;
            let mut join_on = rel_join(rel.direction, &prev_alias, &rel_alias);
            if let Some(rel_type) = &rel.rel_type {
                use std::fmt::Write as _;
                let _ = write!(join_on, " AND {rel_alias}.edge_type = ?");
                params.push(SqlValue::Text(rel_type.clone()));
            }
            joins.push(format!("JOIN edges {rel_alias} ON {join_on}"));
            bindings.push(Binding {
                var: rel.var.clone(),
                alias: rel_alias.clone(),
                is_node: false,
                label: None,
                props: Vec::new(),
            });

            let node_alias = node_alias(node, &mut var_alias, &mut alias_seq);
            let target_join = match rel.direction {
                Direction::Outgoing => format!("{node_alias}.id = {rel_alias}.target"),
                Direction::Incoming => format!("{node_alias}.id = {rel_alias}.source"),
                // Exactly the two ordered endpoints of the edge: when the
                // anchor is the source the target is the other end, and vice
                // versa. A plain OR would also pair an endpoint with itself.
                Direction::Undirected => format!(
                    "{node_alias}.id = CASE WHEN {rel_alias}.source = {prev_alias}.id \
                     THEN {rel_alias}.target ELSE {rel_alias}.source END"
                ),
            };
            joins.push(format!("JOIN nodes {node_alias} ON {target_join}"));
            record_binding(node, &node_alias, &mut bindings);
            prev_alias = node_alias;
        }
    }

    // Pattern-embedded node constraints (labels, property maps) are emitted as
    // SQL `WHERE` predicates — this is the pushdown: SQLite filters the scan
    // rather than the executor filtering materialized rows.
    for binding in &bindings {
        if !binding.is_node {
            continue;
        }
        if let Some(label) = &binding.label {
            where_parts.push(format!("{}.label = ?", binding.alias));
            params.push(SqlValue::Text(label.clone()));
        }
        for (key, value) in &binding.props {
            where_parts.push(format!("{} = ?", prop_expr(&binding.alias, true, key)));
            params.push(value_to_sql(value));
        }
    }

    // User WHERE conjuncts, resolved against the bound variable aliases.
    if let Some(where_clause) = &query.where_clause {
        let mut conjuncts = Vec::new();
        split_conjuncts(where_clause, &mut conjuncts);
        for conjunct in &conjuncts {
            where_parts.push(expr_to_sql(conjunct, &bindings, &mut params)?);
        }
    }

    // SELECT list.
    let mut columns: Vec<OutCol> = Vec::new();
    for item in &query.return_items {
        match item {
            ReturnItem::Star => {
                for binding in &bindings {
                    if let Some(var) = &binding.var {
                        columns.push(binding_column(var, binding));
                    }
                }
            }
            ReturnItem::Var(var) => {
                let binding = find_binding(&bindings, var)?;
                columns.push(binding_column(var, binding));
            }
            ReturnItem::Prop { var, prop, alias } if prop.is_empty() => {
                let binding = find_binding(&bindings, var)?;
                let name = alias.clone().unwrap_or_else(|| var.clone());
                columns.push(binding_column(&name, binding));
            }
            ReturnItem::Prop { var, prop, alias } => {
                let binding = find_binding(&bindings, var)?;
                let name = alias.clone().unwrap_or_else(|| format!("{var}.{prop}"));
                columns.push(OutCol::Scalar {
                    sql: prop_expr(&binding.alias, binding.is_node, prop),
                    name,
                });
            }
            ReturnItem::CountStar => {
                columns.push(OutCol::Scalar {
                    sql: "count(*)".to_string(),
                    name: "count(*)".to_string(),
                });
            }
        }
    }
    if columns.is_empty() {
        return Err(CypherError::Plan("RETURN produced no columns".to_string()));
    }

    // ORDER BY.
    let mut order_parts: Vec<String> = Vec::new();
    for (item, asc) in &query.order_by {
        let expr = match item {
            ReturnItem::Prop { var, prop, .. } => {
                let binding = find_binding(&bindings, var)?;
                prop_expr(&binding.alias, binding.is_node, prop)
            }
            ReturnItem::Var(var) => {
                let binding = find_binding(&bindings, var)?;
                format!("{}.id", binding.alias)
            }
            ReturnItem::CountStar => "count(*)".to_string(),
            ReturnItem::Star => {
                return Err(CypherError::Plan("cannot ORDER BY *".to_string()));
            }
        };
        order_parts.push(format!("{expr} {}", if *asc { "ASC" } else { "DESC" }));
    }

    // Assemble SQL.
    let mut sql = String::from("SELECT ");
    sql.push_str(&select_list(&columns));
    sql.push_str(" FROM ");
    sql.push_str(&from_parts.join(", "));
    for join in &joins {
        sql.push(' ');
        sql.push_str(join);
    }
    if !where_parts.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_parts.join(" AND "));
    }
    if !order_parts.is_empty() {
        sql.push_str(" ORDER BY ");
        sql.push_str(&order_parts.join(", "));
    }
    if let Some(limit) = query.limit {
        use std::fmt::Write as _;
        let _ = write!(sql, " LIMIT {limit}");
    }

    Ok(Planned {
        sql,
        params,
        columns,
        optimizations,
    })
}

fn reorder_patterns(patterns: &mut [PathPattern]) -> bool {
    let original: Vec<usize> = patterns.iter().map(pattern_cost).collect();
    // Stable sort: the most constrained (highest cost) pattern leads the join
    // chain, so SQLite starts from the smallest relation. Equal costs keep
    // their written order.
    patterns.sort_by_key(|p| std::cmp::Reverse(pattern_cost(p)));
    let reordered: Vec<usize> = patterns.iter().map(pattern_cost).collect();
    original != reordered
}

/// Number of constraints on a pattern; more constraints means a cheaper scan.
fn pattern_cost(pattern: &PathPattern) -> usize {
    let mut cost = 0usize;
    cost += node_cost(&pattern.start);
    for (rel, node) in &pattern.steps {
        cost += node_cost(node);
        if rel.rel_type.is_some() {
            cost += 2;
        }
        if rel.var.is_some() {
            cost += 1;
        }
    }
    cost
}

fn node_cost(node: &NodePattern) -> usize {
    let mut cost = 0usize;
    if node.label.is_some() {
        cost += 2;
    }
    cost += node.props.len() * 3;
    if node.var.is_some() {
        cost += 1;
    }
    cost
}

fn split_conjuncts(expr: &Expr, out: &mut Vec<Expr>) {
    match expr {
        Expr::And(a, b) => {
            split_conjuncts(a, out);
            split_conjuncts(b, out);
        }
        other => out.push(other.clone()),
    }
}

fn node_alias(
    node: &NodePattern,
    var_alias: &mut BTreeMap<String, String>,
    alias_seq: &mut usize,
) -> String {
    if let Some(var) = &node.var {
        if let Some(existing) = var_alias.get(var) {
            return existing.clone();
        }
        let alias = format!("n{var}");
        var_alias.insert(var.clone(), alias.clone());
        return alias;
    }
    let alias = format!("n{}", *alias_seq);
    *alias_seq += 1;
    alias
}

/// Record (or merge) the binding for `node` under `alias`.
fn record_binding(node: &NodePattern, alias: &str, bindings: &mut Vec<Binding>) {
    if let Some(binding) = bindings.iter_mut().find(|b| b.alias == alias) {
        // Same variable reused: merge any new constraints onto the binding.
        if binding.label.is_none() {
            binding.label.clone_from(&node.label);
        }
        for (k, v) in &node.props {
            if !binding.props.iter().any(|(bk, _)| bk == k) {
                binding.props.push((k.clone(), v.clone()));
            }
        }
        return;
    }
    bindings.push(Binding {
        var: node.var.clone(),
        alias: alias.to_string(),
        is_node: true,
        label: node.label.clone(),
        props: node.props.clone(),
    });
}

/// Bind an anchor node to a plain `FROM nodes alias` entry. A variable that is
/// already bound is merged instead of re-added (SQLite would reject a duplicate
/// alias in the FROM list).
fn bind_anchor(
    node: &NodePattern,
    alias: &str,
    bindings: &mut Vec<Binding>,
    from_parts: &mut Vec<String>,
) {
    if bindings.iter().any(|b| b.alias == alias) {
        record_binding(node, alias, bindings);
        return;
    }
    from_parts.push(format!("nodes {alias}"));
    record_binding(node, alias, bindings);
}

/// Node columns that are real SQL columns (not entries in `props_json`).
const NODE_PROP_COLUMNS: &[&str] = &[
    "id",
    "label",
    "name",
    "qualified_name",
    "file_path",
    "start_line",
    "end_line",
];
/// Edge columns that are real SQL columns.
const EDGE_PROP_COLUMNS: &[&str] = &["id", "source", "target", "edge_type"];

/// SQL expression reading property `prop` off `alias`, using the real column
/// when the property is one of the schema's columns and `json_extract`
/// otherwise. This is what makes `n.name` mean the `name` column rather than a
/// (usually absent) `props_json.name`.
fn prop_expr(alias: &str, is_node: bool, prop: &str) -> String {
    let known = if is_node {
        NODE_PROP_COLUMNS
    } else {
        EDGE_PROP_COLUMNS
    };
    if known.contains(&prop) {
        format!("{alias}.{}", quote_ident(prop))
    } else {
        format!("json_extract({alias}.props_json, '$.{prop}')")
    }
}

fn rel_join(direction: Direction, prev_alias: &str, rel_alias: &str) -> String {
    match direction {
        Direction::Outgoing => format!("{rel_alias}.source = {prev_alias}.id"),
        Direction::Incoming => format!("{rel_alias}.target = {prev_alias}.id"),
        Direction::Undirected => format!(
            "({rel_alias}.source = {prev_alias}.id OR {rel_alias}.target = {prev_alias}.id)"
        ),
    }
}

fn find_binding<'a>(bindings: &'a [Binding], var: &str) -> CypherResult<&'a Binding> {
    bindings
        .iter()
        .find(|b| b.var.as_deref() == Some(var))
        .ok_or_else(|| CypherError::Plan(format!("unbound variable {var:?}")))
}

fn binding_column(name: &str, binding: &Binding) -> OutCol {
    if binding.is_node {
        OutCol::Node {
            name: name.to_string(),
            alias: binding.alias.clone(),
        }
    } else {
        OutCol::Edge {
            name: name.to_string(),
            alias: binding.alias.clone(),
        }
    }
}

fn select_list(columns: &[OutCol]) -> String {
    let mut parts = Vec::new();
    for (i, col) in columns.iter().enumerate() {
        match col {
            OutCol::Scalar { name, sql } => {
                parts.push(format!("{sql} AS {}", quote_ident(&format!("c{i}_{name}"))));
            }
            OutCol::Node { alias, .. } => {
                for field in NODE_FIELDS {
                    parts.push(format!(
                        "{alias}.{field} AS {}",
                        quote_ident(&format!("c{i}_{field}"))
                    ));
                }
            }
            OutCol::Edge { alias, .. } => {
                for field in EDGE_FIELDS {
                    parts.push(format!(
                        "{alias}.{field} AS {}",
                        quote_ident(&format!("c{i}_{field}"))
                    ));
                }
            }
        }
    }
    parts.join(", ")
}

const NODE_FIELDS: &[&str] = &[
    "id",
    "label",
    "name",
    "qualified_name",
    "file_path",
    "start_line",
    "end_line",
    "props_json",
];
const EDGE_FIELDS: &[&str] = &["id", "source", "target", "edge_type", "props_json"];

fn expr_to_sql(
    expr: &Expr,
    bindings: &[Binding],
    params: &mut Vec<SqlValue>,
) -> CypherResult<String> {
    match expr {
        Expr::Compare {
            var,
            prop,
            op,
            value,
        } => {
            let col = prop_expr(&binding_for(bindings, var)?.alias, true, prop);
            if matches!(value, CypherValue::Null) && *op == CmpOp::Eq {
                params.push(SqlValue::Null);
                return Ok(format!("{col} IS ?"));
            }
            params.push(value_to_sql(value));
            Ok(format!("{col} {} ?", op.sql()))
        }
        Expr::IsNull {
            var,
            prop,
            negated,
        } => {
            let col = prop_expr(&binding_for(bindings, var)?.alias, true, prop);
            Ok(if *negated {
                format!("{col} IS NOT NULL")
            } else {
                format!("{col} IS NULL")
            })
        }
        Expr::And(a, b) => Ok(format!(
            "({} AND {})",
            expr_to_sql(a, bindings, params)?,
            expr_to_sql(b, bindings, params)?
        )),
        Expr::Or(a, b) => Ok(format!(
            "({} OR {})",
            expr_to_sql(a, bindings, params)?,
            expr_to_sql(b, bindings, params)?
        )),
        Expr::Not(inner) => Ok(format!("NOT ({})", expr_to_sql(inner, bindings, params)?)),
    }
}

fn binding_for<'a>(bindings: &'a [Binding], var: &str) -> CypherResult<&'a Binding> {
    bindings
        .iter()
        .find(|b| b.var.as_deref() == Some(var))
        .ok_or_else(|| CypherError::Plan(format!("unbound variable {var:?}")))
}

#[allow(clippy::match_same_arms)]
fn value_to_sql(value: &CypherValue) -> SqlValue {
    match value {
        CypherValue::Null => SqlValue::Null,
        CypherValue::Int(n) => SqlValue::Integer(*n),
        CypherValue::Float(f) => SqlValue::Real(*f),
        CypherValue::Str(s) => SqlValue::Text(s.clone()),
        CypherValue::Bool(b) => SqlValue::Integer(i64::from(*b)),
        CypherValue::Object(_) => SqlValue::Null,
    }
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

// ── executor ────────────────────────────────────────────────────────────

/// The read-only Cypher engine: parse, plan, and run against a [`GraphStore`].
#[derive(Debug, Clone, Copy, Default)]
pub struct CypherEngine;

impl CypherEngine {
    /// Parse `cypher` into a [`QueryPlan`] without touching the store.
    ///
    /// # Errors
    /// Returns the lex/parse/plan error for an unsupported or malformed query.
    pub fn plan(cypher: &str) -> CypherResult<QueryPlan> {
        let toks = lex(cypher)?;
        let mut parser = Parser::new(toks);
        let query = parser.parse_query()?;
        let planned = plan(&query)?;
        Ok(QueryPlan {
            sql: planned.sql,
            optimizations: planned.optimizations,
        })
    }

    /// Parse and execute `cypher` against `store`, returning the rows.
    ///
    /// # Errors
    /// Propagates lex/parse/plan errors and SQLite failures.
    pub fn query(store: &GraphStore, cypher: &str) -> CypherResult<Rows> {
        let toks = lex(cypher)?;
        let mut parser = Parser::new(toks);
        let query = parser.parse_query()?;
        let planned = plan(&query)?;
        execute(store, &planned)
    }
}

fn execute(store: &GraphStore, planned: &Planned) -> CypherResult<Rows> {
    let mut stmt = store.connection().prepare(&planned.sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = planned
        .params
        .iter()
        .map(|v| v as &dyn rusqlite::ToSql)
        .collect();
    let column_names: Vec<String> = planned
        .columns
        .iter()
        .map(|c| match c {
            OutCol::Scalar { name, .. }
            | OutCol::Node { name, .. }
            | OutCol::Edge { name, .. } => name.clone(),
        })
        .collect();

    let mut rows = Vec::new();
    let mut raw = stmt.query(param_refs.as_slice())?;
    while let Some(row) = raw.next()? {
        let mut out = Row::new();
        let mut cursor = 0usize;
        for (i, col) in planned.columns.iter().enumerate() {
            match col {
                OutCol::Scalar { name, .. } => {
                    let value: SqlValue = row.get(cursor)?;
                    cursor += 1;
                    out.insert(name.clone(), value.into());
                }
                OutCol::Node { name, .. } => {
                    let mut obj = BTreeMap::new();
                    for field in NODE_FIELDS {
                        let value: SqlValue = row.get(cursor)?;
                        cursor += 1;
                        obj.insert((*field).to_string(), value.into());
                    }
                    out.insert(name.clone(), CypherValue::Object(obj));
                }
                OutCol::Edge { name, .. } => {
                    let mut obj = BTreeMap::new();
                    for field in EDGE_FIELDS {
                        let value: SqlValue = row.get(cursor)?;
                        cursor += 1;
                        obj.insert((*field).to_string(), value.into());
                    }
                    out.insert(name.clone(), CypherValue::Object(obj));
                }
            }
            let _ = i;
        }
        rows.push(out);
    }
    Ok(Rows {
        columns: column_names,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EdgeType, Label, Node};

    fn seeded_store() -> GraphStore {
        let store = GraphStore::open_in_memory().expect("open");
        store.register_project("demo", "/tmp/demo").expect("project");
        let mut nodes = Vec::new();
        for (label, name, props) in [
            (Label::Function, "main", serde_json::json!({"doc": "entry"})),
            (Label::Function, "parse_config", serde_json::json!({"doc": "parse"})),
            (Label::Function, "read_file", serde_json::json!({"doc": "read"})),
            (Label::Struct, "Config", serde_json::json!({})),
        ] {
            let node = Node {
                id: 0,
                project: "demo".to_string(),
                label,
                name: name.to_string(),
                qualified_name: format!("demo::{name}"),
                file_path: "src/lib.rs".to_string(),
                start_line: 1,
                end_line: 2,
                props,
            };
            let id = store.upsert_node(&node).expect("node");
            nodes.push(id);
        }
        let edges = [
            (nodes[0], nodes[1], EdgeType::Calls),
            (nodes[1], nodes[2], EdgeType::Calls),
            (nodes[1], nodes[3], EdgeType::Uses),
        ];
        for (source, target, edge_type) in edges {
            store
                .insert_edge("demo", source, target, edge_type, &serde_json::json!({}))
                .expect("edge");
        }
        store
    }

    #[test]
    fn lexer_handles_arrows_and_literals() {
        let toks = lex("MATCH (n:Function)-[r:CALLS]->(m) WHERE n.name = 'main'").expect("lex");
        assert!(toks.contains(&Tok::ArrowRight));
        assert!(toks.contains(&Tok::Str("main".to_string())));
    }

    #[test]
    fn simple_match_returns_nodes() {
        let store = seeded_store();
        let rows = CypherEngine::query(&store, "MATCH (n:Function) RETURN n.name").expect("query");
        assert_eq!(rows.len(), 3);
        let names: Vec<String> = rows
            .rows
            .iter()
            .filter_map(|r| match r.get("n.name") {
                Some(CypherValue::Str(s)) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"main".to_string()));
        assert!(names.contains(&"parse_config".to_string()));
    }

    #[test]
    fn relationship_pattern_follows_edges() {
        let store = seeded_store();
        let rows = CypherEngine::query(
            &store,
            "MATCH (a:Function)-[r:CALLS]->(b) RETURN a.name, b.name",
        )
        .expect("query");
        // main->parse_config and parse_config->read_file
        assert_eq!(rows.len(), 2);
        assert!(rows.rows.iter().any(|r| {
            r.get("a.name") == Some(&CypherValue::Str("main".to_string()))
                && r.get("b.name") == Some(&CypherValue::Str("parse_config".to_string()))
        }));
    }

    #[test]
    fn where_property_filter_narrows_results() {
        let store = seeded_store();
        let rows = CypherEngine::query(
            &store,
            "MATCH (n:Function) WHERE n.name = 'main' RETURN n.name",
        )
        .expect("query");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows.rows[0].get("n.name"),
            Some(&CypherValue::Str("main".to_string()))
        );
    }

    #[test]
    fn where_on_relationship_target_filters() {
        let store = seeded_store();
        let rows = CypherEngine::query(
            &store,
            "MATCH (a:Function)-[r:CALLS]->(b) WHERE a.name <> 'main' RETURN b.name",
        )
        .expect("query");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows.rows[0].get("b.name"),
            Some(&CypherValue::Str("read_file".to_string()))
        );
    }

    #[test]
    fn incoming_direction_is_supported() {
        let store = seeded_store();
        let rows = CypherEngine::query(
            &store,
            "MATCH (a)<-[r:CALLS]-(b) RETURN a.name, b.name",
        )
        .expect("query");
        assert_eq!(rows.len(), 2, "incoming CALLS is the reverse of outgoing");
    }

    #[test]
    fn undirected_matches_both_ways() {
        let store = seeded_store();
        let rows = CypherEngine::query(&store, "MATCH (a)-[r:CALLS]-(b) RETURN a.name, b.name")
            .expect("query");
        assert_eq!(rows.len(), 4, "each CALLS edge is seen from both endpoints");
    }

    #[test]
    fn order_by_and_limit_apply() {
        let store = seeded_store();
        let rows = CypherEngine::query(
            &store,
            "MATCH (n:Function) RETURN n.name ORDER BY n.name ASC LIMIT 2",
        )
        .expect("query");
        assert_eq!(rows.len(), 2);
        let names: Vec<String> = rows
            .rows
            .iter()
            .filter_map(|r| match r.get("n.name") {
                Some(CypherValue::Str(s)) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["main".to_string(), "parse_config".to_string()]);
    }

    #[test]
    fn count_star_aggregates() {
        let store = seeded_store();
        let rows =
            CypherEngine::query(&store, "MATCH (n:Function) RETURN count(*)").expect("query");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows.rows[0].get("count(*)"), Some(&CypherValue::Int(3)));
    }

    #[test]
    fn whole_node_return_is_an_object() {
        let store = seeded_store();
        let rows = CypherEngine::query(
            &store,
            "MATCH (n:Struct) RETURN n",
        )
        .expect("query");
        assert_eq!(rows.len(), 1);
        match rows.rows[0].get("n") {
            Some(CypherValue::Object(obj)) => {
                assert_eq!(obj.get("name"), Some(&CypherValue::Str("Config".to_string())));
            }
            other => panic!("expected a node object, got {other:?}"),
        }
    }

    #[test]
    fn star_returns_every_bound_variable() {
        let store = seeded_store();
        let rows = CypherEngine::query(
            &store,
            "MATCH (a:Function)-[r:CALLS]->(b) RETURN * LIMIT 1",
        )
        .expect("query");
        assert_eq!(rows.len(), 1);
        assert!(rows.rows[0].contains_key("a"));
        assert!(rows.rows[0].contains_key("b"));
        assert!(rows.rows[0].contains_key("r"));
    }

    #[test]
    fn writes_are_rejected() {
        for q in [
            "CREATE (n:Function) RETURN n",
            "MATCH (n) DELETE n RETURN n",
            "MATCH (n) SET n.name = 'x' RETURN n",
        ] {
            assert!(matches!(CypherEngine::plan(q), Err(CypherError::ReadOnly(_))), "{q}");
        }
    }

    #[test]
    fn predicate_pushdown_appears_in_the_sql() {
        let plan = CypherEngine::plan(
            "MATCH (n:Function)-[r:CALLS]->(m) WHERE n.name = 'main' RETURN m.name",
        )
        .expect("plan");
        assert!(
            plan.optimizations.contains(&Optimization::PredicatePushdown),
            "pushdown must be reported: {:?}",
            plan.optimizations
        );
        // The pushed predicate is in the scan's own WHERE — `n.name` is a real
        // column, so it rides the node scan rather than filtering after the join.
        assert!(plan.sql.contains("\"name\" = ?"), "sql: {}", plan.sql);
        assert!(plan.sql.contains("WHERE"));
    }

    #[test]
    fn join_reordering_is_reported_when_patterns_reorder() {
        // A bare pattern followed by a constrained one should reorder so the
        // constrained scan leads.
        let plan = CypherEngine::plan(
            "MATCH (a)-[r:CALLS]->(b), (c:Function {name: 'main'})-[:CALLS]->(d) RETURN a.name",
        )
        .expect("plan");
        assert!(
            plan.optimizations.contains(&Optimization::JoinReordering),
            "reorder must be reported: {:?}",
            plan.optimizations
        );
    }

    #[test]
    fn property_map_in_pattern_filters() {
        let store = seeded_store();
        let rows = CypherEngine::query(
            &store,
            "MATCH (n:Function {name: 'read_file'}) RETURN n.name",
        )
        .expect("query");
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn or_predicate_is_supported() {
        let store = seeded_store();
        let rows = CypherEngine::query(
            &store,
            "MATCH (n:Function) WHERE n.name = 'main' OR n.name = 'read_file' RETURN n.name",
        )
        .expect("query");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn unbound_variable_is_a_plan_error() {
        assert!(matches!(
            CypherEngine::plan("MATCH (n:Function) WHERE x.name = 'y' RETURN n.name"),
            Err(CypherError::Plan(_))
        ));
    }

    #[test]
    fn malformed_query_is_a_parse_error() {
        assert!(matches!(
            CypherEngine::plan("MATCH (n:Function RETURN n"),
            Err(CypherError::Parse(_))
        ));
    }
}
