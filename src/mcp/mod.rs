//! MCP server: tool table, validation, handlers.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::{config, trello, util};

mod board;
mod card;
mod extras;
mod handlers;
mod server;
#[cfg(test)]
pub(crate) mod test_support;
mod tools;

pub type Args = Map<String, Value>;

#[derive(Debug)]
pub enum Output {
    Json(Value),
    Text(String),
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Trello(#[from] trello::Error),
    #[error(transparent)]
    Config(#[from] config::Error),
    #[error("{0}")]
    Message(String),
}

pub(crate) fn msg(text: impl Into<String>) -> Error {
    Error::Message(text.into())
}

#[derive(Clone)]
pub struct Server {
    pub(crate) client: Arc<trello::Client>,
}

impl Server {
    pub fn new(client: Arc<trello::Client>) -> Self {
        Self { client }
    }
}

pub(crate) fn text(args: &Args, name: &str) -> String {
    args.get(name)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

pub(crate) fn text_default(args: &Args, name: &str, fallback: &str) -> String {
    let value = text(args, name);
    if value.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

pub(crate) fn integer(args: &Args, name: &str, fallback: i64) -> i64 {
    // `as` truncates toward zero like Go's int(float64).
    args.get(name)
        .and_then(Value::as_f64)
        .map_or(fallback, |number| number as i64)
}

pub(crate) fn boolean(args: &Args, name: &str, fallback: bool) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(fallback)
}

/// Pairs are (input, output).
pub(crate) fn map_args(args: &Args, names: &[(&str, &str)]) -> Args {
    let mut result = Args::new();
    for (input, output) in names {
        copy_arg(&mut result, output, args, input);
    }
    result
}

pub(crate) fn copy_arg(target: &mut Args, output: &str, args: &Args, input: &str) {
    if let Some(value) = args.get(input).filter(|value| !value.is_null()) {
        target.insert(output.to_string(), value.clone());
    }
}

pub(crate) fn nested_value<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for name in path {
        current = current.as_object()?.get(*name)?;
    }
    Some(current)
}

pub(crate) fn nested_string(value: &Value, path: &[&str]) -> String {
    nested_value(value, path)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// String at `path` inside an object, or "".
pub(crate) fn object_string(object: &Args, path: &[&str]) -> String {
    match path.split_first() {
        Some((first, rest)) => object
            .get(*first)
            .map(|value| nested_string(value, rest))
            .unwrap_or_default(),
        None => String::new(),
    }
}

pub(crate) fn object_slice(value: &Value) -> Vec<Args> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_object().cloned())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn string_slice(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn copy_present(target: &mut Args, source: &Args, fields: &[&str]) {
    for field in fields {
        match source.get(*field) {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) if text.is_empty() => {}
            Some(value) => {
                target.insert((*field).to_string(), value.clone());
            }
        }
    }
}

pub(crate) fn compact_objects(value: &Value, fields: &[&str]) -> Vec<Value> {
    object_slice(value)
        .iter()
        .map(|object| {
            let mut compact = Args::new();
            copy_present(&mut compact, object, fields);
            Value::Object(compact)
        })
        .collect()
}

pub(crate) fn escape(value: &str) -> String {
    util::path_escape(value)
}
