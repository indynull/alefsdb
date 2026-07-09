//! AlefQL: structured query language for alefsdb.

mod ast;
mod eval;
mod parse;

#[cfg(test)]
mod golden;

pub use ast::{CmpOp, Expr, Predicate, QueryHit};
pub use eval::execute;
pub use parse::{parse_query, ParseError};
