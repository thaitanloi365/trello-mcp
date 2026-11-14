//! In-place JSON(C) patcher: sets one entry and leaves every other byte of the
//! document (comments, key order, formatting) untouched.

use serde_json::Value;

use super::{Error, go_escape};

const MAX_DEPTH: usize = 128;

type Res<T> = Result<T, Error>;

fn bad(msg: &str) -> Error {
    Error::Invalid(format!("cannot parse JSON: {msg}"))
}

struct Member {
    kstart: usize,
    key: String,
    vstart: usize,
    vend: usize,
}

struct Ctx<'a> {
    eol: &'a str,
    unit: String,
}

fn skip_ws(b: &[u8], mut i: usize) -> Res<usize> {
    loop {
        match (b.get(i), b.get(i + 1)) {
            (Some(c), _) if c.is_ascii_whitespace() => i += 1,
            (Some(b'/'), Some(b'/')) => {
                while b.get(i).is_some_and(|c| *c != b'\n') {
                    i += 1;
                }
            }
            (Some(b'/'), Some(b'*')) => {
                i += 2;
                while b.get(i..i + 2) != Some(b"*/") {
                    if i >= b.len() {
                        return Err(bad("unterminated comment"));
                    }
                    i += 1;
                }
                i += 2;
            }
            _ => return Ok(i),
        }
    }
}

fn string_end(b: &[u8], mut i: usize) -> Res<usize> {
    i += 1;
    while let Some(c) = b.get(i) {
        match c {
            b'\\' => i += 2,
            b'"' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err(bad("unterminated string"))
}

fn value_end(b: &[u8], i: usize, depth: usize) -> Res<usize> {
    if depth > MAX_DEPTH {
        return Err(bad("nested deeper than 128 levels"));
    }
    match b.get(i) {
        Some(b'"') => string_end(b, i),
        Some(b'{') => Ok(members(b, i, depth + 1)?.1 + 1),
        Some(b'[') => {
            let mut j = skip_ws(b, i + 1)?;
            while b.get(j) != Some(&b']') {
                j = skip_ws(b, value_end(b, j, depth + 1)?)?;
                if b.get(j) == Some(&b',') {
                    j = skip_ws(b, j + 1)?;
                } else if b.get(j) != Some(&b']') {
                    return Err(bad("expected , or ]"));
                }
            }
            Ok(j + 1)
        }
        Some(_) => {
            let mut j = i;
            while b
                .get(j)
                .is_some_and(|c| !c.is_ascii_whitespace() && !b",}]/".contains(c))
            {
                j += 1;
            }
            // Only true, false, null and well-formed numbers are scalars.
            if j == i || serde_json::from_slice::<Value>(&b[i..j]).is_err() {
                return Err(bad("invalid value"));
            }
            Ok(j)
        }
        None => Err(bad("unexpected end of file")),
    }
}

/// Members of the object opening at `open`, and the index of its closing brace.
fn members(b: &[u8], open: usize, depth: usize) -> Res<(Vec<Member>, usize)> {
    let mut out = Vec::new();
    let mut i = skip_ws(b, open + 1)?;
    while b.get(i) != Some(&b'}') {
        if b.get(i) != Some(&b'"') {
            return Err(bad("expected a key"));
        }
        let kend = string_end(b, i)?;
        let key = serde_json::from_slice::<String>(&b[i..kend]).map_err(|e| bad(&e.to_string()))?;
        let colon = skip_ws(b, kend)?;
        if b.get(colon) != Some(&b':') {
            return Err(bad("expected :"));
        }
        let vstart = skip_ws(b, colon + 1)?;
        let vend = value_end(b, vstart, depth)?;
        out.push(Member {
            kstart: i,
            key,
            vstart,
            vend,
        });
        i = skip_ws(b, vend)?;
        if b.get(i) == Some(&b',') {
            i = skip_ws(b, i + 1)?;
        } else if b.get(i) != Some(&b'}') {
            return Err(bad("expected , or }"));
        }
    }
    Ok((out, i))
}

/// Whitespace before `pos` on its line, when the line holds nothing else.
fn own_line_indent(text: &str, pos: usize) -> Option<String> {
    let start = text[..pos].rfind('\n').map_or(0, |n| n + 1);
    let prefix = &text[start..pos];
    prefix.trim().is_empty().then(|| prefix.to_string())
}

/// Leading whitespace of the line holding `pos`.
fn leading_ws(text: &str, pos: usize) -> String {
    let start = text[..pos].rfind('\n').map_or(0, |n| n + 1);
    text[start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

fn splice(text: &str, from: usize, to: usize, with: &str) -> String {
    format!("{}{with}{}", &text[..from], &text[to..])
}

fn pretty(value: &Value, ind: &str, eol: &str) -> String {
    let data = serde_json::to_string_pretty(value).unwrap_or_default();
    go_escape(&data).replace('\n', &format!("{eol}{ind}"))
}

fn nest(path: &[&str], value: &Value) -> Value {
    path.iter().rev().fold(value.clone(), |inner, key| {
        Value::Object([(key.to_string(), inner)].into_iter().collect())
    })
}

/// Root object opening, after checking the document is exactly one object.
fn root(text: &str) -> Res<usize> {
    let b = text.as_bytes();
    let root = skip_ws(b, 0)?;
    if b.get(root) != Some(&b'{') {
        return Err(bad("top level is not an object"));
    }
    let (_, close) = members(b, root, 0)?;
    if skip_ws(b, close + 1)? != b.len() {
        return Err(bad("unexpected content after the top-level object"));
    }
    Ok(root)
}

/// Value span of `path`, or None when a key is missing. A non-object on the
/// way is an error.
fn lookup(text: &str, path: &[&str]) -> Res<Option<(usize, usize)>> {
    let b = text.as_bytes();
    let mut open = root(text)?;
    for (n, key) in path.iter().enumerate() {
        let (ms, _) = members(b, open, 0)?;
        let Some(m) = ms.iter().rev().find(|m| m.key == *key) else {
            return Ok(None);
        };
        if n + 1 == path.len() {
            return Ok(Some((m.vstart, m.vend)));
        }
        if b[m.vstart] != b'{' {
            return Err(Error::Invalid(format!("field {key:?} is not an object")));
        }
        open = m.vstart;
    }
    Ok(None)
}

fn put(text: &str, open: usize, path: &[&str], value: &Value, ctx: &Ctx) -> Res<String> {
    let b = text.as_bytes();
    let (ms, close) = members(b, open, 0)?;
    let key = path[0];
    let ind = ms
        .first()
        .and_then(|m| own_line_indent(text, m.kstart))
        .unwrap_or_else(|| format!("{}{}", leading_ws(text, open), ctx.unit));
    let found = ms.iter().rev().find(|m| m.key == key);
    match found {
        Some(m) if path.len() > 1 => {
            if b[m.vstart] != b'{' {
                return Err(Error::Invalid(format!("field {key:?} is not an object")));
            }
            put(text, m.vstart, &path[1..], value, ctx)
        }
        Some(m) => {
            let ind = own_line_indent(text, m.kstart).unwrap_or(ind);
            Ok(splice(
                text,
                m.vstart,
                m.vend,
                &pretty(value, &ind, ctx.eol),
            ))
        }
        None => {
            let member = format!(
                "{}: {}",
                serde_json::to_string(key).unwrap_or_default(),
                pretty(&nest(&path[1..], value), &ind, ctx.eol)
            );
            let eol = ctx.eol;
            let closing = leading_ws(text, open);
            let Some(last) = ms.last() else {
                let end = if text[open + 1..close].trim().is_empty() {
                    close
                } else {
                    open + 1
                };
                return Ok(splice(
                    text,
                    open + 1,
                    end,
                    &format!("{eol}{ind}{member}{eol}{closing}"),
                ));
            };
            let j = skip_ws(b, last.vend)?;
            Ok(if b[j] == b',' {
                splice(text, j + 1, j + 1, &format!("{eol}{ind}{member}"))
            } else {
                splice(text, last.vend, last.vend, &format!(",{eol}{ind}{member}"))
            })
        }
    }
}

/// The fields that make an entry a URL one.
const URL_KEYS: [&str; 2] = ["url", "serverUrl"];

/// Sets `container.name = entry`. When `name` already holds an object, only
/// the fields of `entry` are written, so the user's own extra keys survive;
/// an existing `enabled` is never overridden.
pub(super) fn patch(
    text: &str,
    container: &str,
    name: &str,
    entry: &Value,
    eol: &str,
) -> Res<String> {
    let open = root(text)?;
    let (top, _) = members(text.as_bytes(), open, 0)?;
    let unit = top
        .first()
        .and_then(|m| own_line_indent(text, m.kstart))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "  ".into());
    let ctx = Ctx { eol, unit };
    let mut out = text.to_string();
    let base = [container, name];
    let existing = lookup(&out, &base)?;
    let (Some((s, _)), Value::Object(fields)) = (existing, entry) else {
        return put(&out, open, &base, entry, &ctx);
    };
    if out.as_bytes()[s] != b'{' {
        return put(&out, open, &base, entry, &ctx);
    }
    // A switch between a command and a URL replaces the entry: the old kind's fields must go.
    let mut was_url = false;
    for k in URL_KEYS {
        was_url |= lookup(&out, &[container, name, k])?.is_some();
    }
    if was_url != URL_KEYS.iter().any(|k| fields.contains_key(*k)) {
        return put(&out, open, &base, entry, &ctx);
    }
    for (k, v) in fields {
        let path = [container, name, k.as_str()];
        if k == "enabled" && lookup(&out, &path)?.is_some() {
            continue;
        }
        out = put(&out, root(&out)?, &path, v, &ctx)?;
    }
    Ok(out)
}

/// Drops each `projects.<dir>.mcpServers.trello` in Claude Code's
/// `~/.claude.json` that only starts this server (`trello-mcp serve`, no
/// config or env). Claude Code lets those local entries win over the user
/// entry, so they would keep a stdio server per session after a switch to the
/// shared one. Entries with their own config, env or command are kept.
pub(super) fn drop_local_trello(text: &str) -> Res<String> {
    let Ok(doc) = serde_json::from_str::<Value>(text) else {
        return Ok(text.to_string());
    };
    let Some(projects) = doc.get("projects").and_then(Value::as_object) else {
        return Ok(text.to_string());
    };
    let mut out = text.to_string();
    for (dir, project) in projects {
        let Some(entry) = project.pointer("/mcpServers/trello") else {
            continue;
        };
        if !plain_stdio(entry) {
            continue;
        }
        let Some(_) = lookup(&out, &["projects", dir, "mcpServers", "trello"])? else {
            continue;
        };
        out = remove(&out, &["projects", dir, "mcpServers"], "trello")?;
    }
    Ok(out)
}

fn plain_stdio(entry: &Value) -> bool {
    let Some(obj) = entry.as_object() else {
        return false;
    };
    let ours = obj
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| std::path::Path::new(c).file_name() == Some("trello-mcp".as_ref()));
    let serve = obj.get("args") == Some(&serde_json::json!(["serve"]));
    let rest = obj.iter().all(|(k, v)| match k.as_str() {
        "type" => v == "stdio",
        "command" | "args" => true,
        "env" => v.as_object().is_some_and(|m| m.is_empty()),
        _ => false,
    });
    ours && serve && rest
}

/// Removes member `name` from the object at `path`, with its comma.
fn remove(text: &str, path: &[&str], name: &str) -> Res<String> {
    let b = text.as_bytes();
    let Some((open, _)) = lookup(text, path)? else {
        return Ok(text.to_string());
    };
    if b[open] != b'{' {
        return Ok(text.to_string());
    }
    let (ms, close) = members(b, open, 0)?;
    let Some(i) = ms.iter().rposition(|m| m.key == name) else {
        return Ok(text.to_string());
    };
    Ok(match (i.checked_sub(1), ms.get(i + 1)) {
        (_, Some(next)) => splice(text, ms[i].kstart, next.kstart, ""),
        (Some(prev), None) => splice(text, ms[prev].vend, ms[i].vend, ""),
        (None, None) => splice(text, open + 1, close, ""),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_only_plain_local_trello_entries() {
        let text = r#"{
  "mcpServers": {"trello": {"type": "http", "url": "u"}},
  "projects": {
    "/a": {"mcpServers": {"trello": {"type": "stdio", "command": "/x/bin/trello-mcp", "args": ["serve"], "env": {}}}, "keep": 1},
    "/b": {"mcpServers": {"other": {"command": "o"}, "trello": {"type": "stdio", "command": "/x/trello-mcp", "args": ["serve"]}}},
    "/c": {"mcpServers": {"trello": {"command": "/x/trello-mcp", "args": ["serve"]}, "z": {}}},
    "/d": {"mcpServers": {"trello": {"command": "/x/trello-mcp", "args": ["serve", "--config", "/p"]}}},
    "/e": {"mcpServers": {"trello": {"command": "/x/trello-mcp", "args": ["serve"], "env": {"K": "v"}}}},
    "/f": {"mcpServers": {"trello": {"command": "/x/other", "args": ["serve"]}}}
  }
}"#;
        let out = drop_local_trello(text).unwrap();
        let doc: Value = serde_json::from_str(&out).unwrap();
        let p = &doc["projects"];
        assert_eq!(p["/a"]["mcpServers"], serde_json::json!({}));
        assert_eq!(p["/a"]["keep"], 1);
        assert_eq!(
            p["/b"]["mcpServers"],
            serde_json::json!({"other": {"command": "o"}})
        );
        assert_eq!(p["/c"]["mcpServers"], serde_json::json!({"z": {}}));
        for kept in ["/d", "/e", "/f"] {
            assert!(p[kept]["mcpServers"].get("trello").is_some(), "{kept}");
        }
        assert_eq!(doc["mcpServers"]["trello"]["url"], "u");
        // Every other line is untouched, and a second pass changes nothing.
        assert!(out.contains(r#""/d": {"mcpServers": {"trello": {"command": "/x/trello-mcp", "args": ["serve", "--config", "/p"]}}},"#));
        assert_eq!(drop_local_trello(&out).unwrap(), out);
    }
}
