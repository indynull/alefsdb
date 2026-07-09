use serde::{Deserialize, Serialize};

/// Client → server request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Mkdir {
        path: String,
    },
    Set {
        path: String,
        #[serde(rename = "type")]
        type_name: String,
        value: String,
    },
    Get {
        path: String,
    },
    Ls {
        path: String,
    },
    Rm {
        path: String,
    },
    Hset {
        path: String,
        key: String,
        #[serde(rename = "type")]
        type_name: String,
        value: String,
    },
    Lpush {
        path: String,
        #[serde(rename = "type")]
        type_name: String,
        value: String,
    },
    Sadd {
        path: String,
        #[serde(rename = "type")]
        type_name: String,
        value: String,
    },
    Tset {
        path: String,
        key: String,
        #[serde(rename = "type")]
        type_name: String,
        value: String,
    },
    Query {
        query: String,
    },
    Compact,
    Export,
    Import {
        json: String,
    },
}

/// Server → client response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { message: String },
    Value { type_name: String, display: String },
    List { entries: Vec<ListEntry> },
    Query { hits: Vec<QueryHitDto> },
    Export { json: String },
    Err { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListEntry {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryHitDto {
    pub path: String,
    pub type_name: String,
}
