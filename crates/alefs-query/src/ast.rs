use alefs_types::{DbPath, Scalar, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Pred(Predicate),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    PathGlob(String),
    TypeName(String),
    NameGlob(String),
    ValueCmp(CmpOp, Scalar),
    HasKey(String),
    Contains(Value),
    AtIndex(u64, CmpOp, Scalar),
    KeyCmp(CmpOp, Scalar),
    SizeCmp(CmpOp, i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryHit {
    pub path: DbPath,
    pub type_name: String,
}
