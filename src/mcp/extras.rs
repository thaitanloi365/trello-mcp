//! Card creation/update, custom fields, attachments, boards, activity, checklists.
//! Everything except `link_attachments` is a stub until Task 7.

use serde_json::Value;

use super::{Args, Error, Server};

impl Server {
    pub(crate) async fn create_cards(&self, _args: &Args) -> Result<Value, Error> {
        todo!("task 7")
    }
    pub(crate) async fn update_card(&self, _args: &Args) -> Result<Value, Error> {
        todo!("task 7")
    }
    pub(crate) async fn set_custom_field(&self, _args: &Args) -> Result<Value, Error> {
        todo!("task 7")
    }
    pub(crate) async fn add_attachment(&self, _args: &Args) -> Result<Value, Error> {
        todo!("task 7")
    }
    pub(crate) async fn activity(&self, _args: &Args) -> Result<Value, Error> {
        todo!("task 7")
    }
    pub(crate) async fn find_checklist_items(&self, _args: &Args) -> Result<Value, Error> {
        todo!("task 7")
    }
    pub(crate) async fn create_checklist(&self, _args: &Args) -> Result<Value, Error> {
        todo!("task 7")
    }
    pub(crate) async fn set_active(
        &self,
        _board_id: &str,
        _workspace_id: &str,
    ) -> Result<Value, Error> {
        todo!("task 7")
    }
    pub(crate) async fn list_boards(&self, _workspace_id: &str) -> Result<Value, Error> {
        todo!("task 7")
    }
    pub(crate) async fn health(&self) -> Result<Value, Error> {
        todo!("task 7")
    }
    pub(crate) fn filter_objects_by_workspace(&self, _value: Value, _field: &str) -> Value {
        todo!("task 7")
    }
}

/// Turns bare attachment URLs into [file name](url) links. Only whole words
/// outside code spans change, so URLs already wrapped in Markdown stay as written.
pub(crate) fn link_attachments(text: &str) -> String {
    text.split('`')
        .enumerate()
        .map(|(index, part)| {
            if index % 2 == 0 {
                link_words(part)
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("`")
}

/// Applies link_attachment to each run of non-space characters. RE2's \s is
/// exactly [\t\n\f\r ].
fn link_words(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    let mut word = String::new();
    for ch in part.chars() {
        if matches!(ch, ' ' | '\t' | '\n' | '\x0C' | '\r') {
            if !word.is_empty() {
                out.push_str(&link_attachment(&word));
                word.clear();
            }
            out.push(ch);
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push_str(&link_attachment(&word));
    }
    out
}

fn link_attachment(word: &str) -> String {
    let link = word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
    let Some(file) = attachment_file(link) else {
        return word.to_string();
    };
    if file.contains("://") {
        return word.to_string();
    }
    let name = crate::util::path_unescape(file).unwrap_or_else(|_| file.to_string());
    let target = link.replace('(', "%28").replace(')', "%29");
    format!("[{name}]({target}){}", &word[link.len()..])
}

/// Matches ^https://trello\.com/1/cards/\w+/attachments/\w+/download/(\S+)$
/// and returns the captured file part.
fn attachment_file(link: &str) -> Option<&str> {
    let rest = link.strip_prefix("https://trello.com/1/cards/")?;
    let (card, rest) = rest.split_once("/attachments/")?;
    let (attachment, file) = rest.split_once("/download/")?;
    let word = |part: &str| {
        !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    (word(card) && word(attachment) && !file.is_empty()).then_some(file)
}
