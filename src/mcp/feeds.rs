//! Trello data for the dashboard: cards in "Ready for dev" and "In progress" lists, the @mentions
//! you have not answered, and card search. Server methods, not MCP tools: the tool list stays lean.

use std::collections::HashMap;

use reqwest::Method;
use serde_json::{Value, json};

use super::{Error, Server, escape, msg, nested_string, object_slice};
use crate::util::ago;

/// The card fields every view needs.
const CARD_FIELDS: &str =
    "name,idShort,shortUrl,shortLink,idList,idBoard,idMembers,dateLastActivity,pos";
pub(crate) const WAITING_DAYS: i64 = 30;
const MENTION_CHARS: usize = 280;
const QUERY_CHARS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListKind {
    ReadyForDev,
    InProgress,
}

/// "Ready for Development", "READY FOR DEV 🚀"; "In-Progress (BE)": case, spaces and marks aside.
pub(crate) fn list_kind(name: &str) -> Option<ListKind> {
    let key: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if key.starts_with("readyfordev") {
        Some(ListKind::ReadyForDev)
    } else if key.starts_with("inprogress") {
        Some(ListKind::InProgress)
    } else {
        None
    }
}

/// Seconds since 1970 of a Trello date ("2026-10-16T03:12:45.123Z"); None when it is not one.
pub(crate) fn unix_seconds(iso: &str) -> Option<i64> {
    let b = iso.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let n = |from: usize, to: usize| iso.get(from..to)?.parse::<i64>().ok();
    let (y, m, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (hh, mm, ss) = (n(11, 13)?, n(14, 16)?, n(17, 19)?);
    // Days from the civil date: Howard Hinnant's algorithm.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// The ISO date Trello takes for `since`.
pub(crate) fn iso_date(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// id → `field` of a board's lists or members.
fn by_id(board: &Value, key: &str, field: &str) -> HashMap<String, String> {
    object_slice(board.get(key).unwrap_or(&Value::Null))
        .iter()
        .map(|o| {
            let get = |k: &str| {
                o.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            (get("id"), get(field))
        })
        .collect()
}

fn cards_of(board: &Value) -> impl Iterator<Item = &Value> {
    board
        .get("cards")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// A card as the page shows it, with its list name and member initials from its board.
pub(crate) fn card_view(card: &Value, board: &Value, now: i64) -> Value {
    let lists = by_id(board, "lists", "name");
    let initials = by_id(board, "members", "initials");
    let members: Vec<String> = card
        .get("idMembers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|id| initials.get(id).cloned())
        .collect();
    let activity = unix_seconds(&nested_string(card, &["dateLastActivity"]))
        .map_or_else(String::new, |t| ago((now - t) as f64));
    json!({
        "id": nested_string(card, &["id"]),
        "number": card.get("idShort").and_then(Value::as_i64),
        "name": nested_string(card, &["name"]),
        "url": nested_string(card, &["shortUrl"]),
        "board": nested_string(board, &["name"]),
        "list": lists.get(&nested_string(card, &["idList"])).cloned().unwrap_or_default(),
        "members": members,
        "activity": activity,
    })
}

/// The cards in every list of this kind, by board (by name); in list order, then card order.
pub(crate) fn named_lists(boards: &[Value], kind: ListKind, now: i64) -> Vec<Value> {
    let mut out: Vec<Value> = boards
        .iter()
        .filter_map(|board| {
            let lists: Vec<String> = object_slice(board.get("lists").unwrap_or(&Value::Null))
                .iter()
                .filter(|l| {
                    list_kind(l.get("name").and_then(Value::as_str).unwrap_or_default())
                        == Some(kind)
                })
                .filter_map(|l| l.get("id").and_then(Value::as_str).map(str::to_string))
                .collect();
            let at = |c: &Value| {
                lists
                    .iter()
                    .position(|l| *l == nested_string(c, &["idList"]))
            };
            let pos = |c: &Value| c.get("pos").and_then(Value::as_f64).unwrap_or_default();
            let mut cards: Vec<&Value> = cards_of(board).filter(|c| at(c).is_some()).collect();
            if cards.is_empty() {
                return None;
            }
            cards.sort_by(|a, b| at(a).cmp(&at(b)).then(pos(a).total_cmp(&pos(b))));
            Some(json!({
                "board": nested_string(board, &["name"]),
                "url": nested_string(board, &["url"]),
                "cards": cards.into_iter().map(|c| card_view(c, board, now)).collect::<Vec<_>>(),
            }))
        })
        .collect();
    out.sort_by_key(|b| nested_string(b, &["board"]).to_lowercase());
    out
}

/// The pure part of `waiting`: mentions of the last 30 days on open cards in `boards` with no
/// comment of yours on that card since; one row per card (its latest mention), newest first.
pub(crate) fn waiting_rows(notes: &Value, mine: &Value, boards: &[Value], now: i64) -> Vec<Value> {
    let since = now - WAITING_DAYS * 86_400;
    let mut my_last: HashMap<String, i64> = HashMap::new();
    for a in object_slice(mine) {
        let a = Value::Object(a);
        if let Some(t) = unix_seconds(&nested_string(&a, &["date"])) {
            let last = my_last
                .entry(nested_string(&a, &["data", "card", "id"]))
                .or_insert(t);
            *last = (*last).max(t);
        }
    }
    let cards: HashMap<String, (&Value, &Value)> = boards
        .iter()
        .flat_map(|b| cards_of(b).map(move |c| (nested_string(c, &["id"]), (c, b))))
        .collect();
    let mut by_card: HashMap<String, Vec<(i64, Value)>> = HashMap::new();
    for n in object_slice(notes) {
        let n = Value::Object(n);
        let card_id = nested_string(&n, &["data", "card", "id"]);
        let Some(t) = unix_seconds(&nested_string(&n, &["date"])) else {
            continue;
        };
        // Archived or out-of-scope cards are not in `cards`.
        if t < since
            || !cards.contains_key(&card_id)
            || my_last.get(&card_id).is_some_and(|m| *m > t)
        {
            continue;
        }
        by_card.entry(card_id).or_default().push((t, n));
    }
    let mut rows: Vec<(i64, Value)> = by_card
        .into_iter()
        .map(|(card_id, mut mentions)| {
            mentions.sort_by_key(|(t, _)| -t);
            let (t, n) = &mentions[0];
            let (card, board) = cards[&card_id];
            let text = nested_string(n, &["data", "text"]);
            let mention = if text.chars().count() > MENTION_CHARS {
                text.chars().take(MENTION_CHARS - 1).chain(['…']).collect()
            } else {
                text
            };
            let action = nested_string(n, &["idAction"]);
            let url = nested_string(card, &["shortUrl"]);
            let comment_url = if action.is_empty() {
                url
            } else {
                format!(
                    "https://trello.com/c/{}#comment-{action}",
                    nested_string(card, &["shortLink"])
                )
            };
            let mut row = card_view(card, board, now);
            if let Some(r) = row.as_object_mut() {
                r.insert("mention".into(), json!(mention));
                r.insert(
                    "by".into(),
                    json!(nested_string(n, &["memberCreator", "fullName"])),
                );
                r.insert("age".into(), json!(ago((now - t) as f64)));
                r.insert("commentUrl".into(), json!(comment_url));
                r.insert("earlier".into(), json!(mentions.len() - 1));
            }
            (*t, row)
        })
        .collect();
    rows.sort_by_key(|(t, _)| -t);
    rows.into_iter().map(|(_, r)| r).collect()
}

/// What a search box query asks for.
#[derive(Debug)]
pub(crate) enum Query {
    Empty,
    /// A card's short link, from its URL or typed bare.
    Link(String),
    /// A card number, on any board.
    Number(i64),
    Text(String),
}

pub(crate) fn classify(q: &str) -> Result<Query, Error> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(Query::Empty);
    }
    if q.chars().count() > QUERY_CHARS {
        return Err(msg(format!(
            "search is limited to {QUERY_CHARS} characters"
        )));
    }
    if let Some((_, rest)) = q.split_once("trello.com/c/") {
        let link: String = rest
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        if !link.is_empty() {
            return Ok(Query::Link(link));
        }
    }
    let digits = q.strip_prefix('#').unwrap_or(q);
    if !digits.is_empty() && digits.len() <= 7 && digits.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(Query::Number(digits.parse().unwrap_or_default()));
    }
    // Short links are 8 random letters and digits; a plain 8-letter word stays a word.
    let b = q.as_bytes();
    if b.len() == 8
        && b.iter().all(u8::is_ascii_alphanumeric)
        && b.iter().any(u8::is_ascii_digit)
        && b.iter().any(u8::is_ascii_alphabetic)
    {
        return Ok(Query::Link(q.to_string()));
    }
    Ok(Query::Text(q.to_string()))
}

/// Cards in the snapshot that `keep` picks, as views.
fn find(boards: &[Value], now: i64, keep: impl Fn(&Value) -> bool) -> Vec<Value> {
    boards
        .iter()
        .flat_map(|b| {
            cards_of(b)
                .filter(|c| keep(c))
                .map(move |c| card_view(c, b, now))
        })
        .collect()
}

impl Server {
    /// One board as the dashboard reads it: lists, open cards and members, in one call.
    pub(crate) async fn board_snapshot(&self, board_id: &str) -> Result<Value, Error> {
        Ok(self
            .client
            .call(
                Method::GET,
                &format!("/boards/{}", escape(board_id)),
                json!({
                    "fields": "name,url",
                    "lists": "open", "list_fields": "name",
                    "cards": "open", "card_fields": CARD_FIELDS,
                    "members": "all", "member_fields": "initials,fullName",
                }),
                None,
            )
            .await?)
    }

    /// The @mentions waiting on you, on the open cards of `boards` (their snapshots).
    pub(crate) async fn waiting(&self, boards: &[Value], now: i64) -> Result<Vec<Value>, Error> {
        let notes = self
            .client
            .call(
                Method::GET,
                "/members/me/notifications",
                json!({"filter": "mentionedOnCard", "limit": 200, "memberCreator": true, "memberCreator_fields": "fullName"}),
                None,
            )
            .await?;
        let mine = self
            .client
            .call(
                Method::GET,
                "/members/me/actions",
                json!({"filter": "commentCard", "since": iso_date(now - WAITING_DAYS * 86_400), "limit": 1000, "fields": "date,data"}),
                None,
            )
            .await?;
        Ok(waiting_rows(&notes, &mine, boards, now))
    }

    /// Card numbers and links resolve from the snapshot; words go to Trello's search on the same boards.
    pub(crate) async fn search_cards(
        &self,
        q: &str,
        boards: &[Value],
        now: i64,
    ) -> Result<Vec<Value>, Error> {
        let text = match classify(q)? {
            Query::Empty => return Ok(Vec::new()),
            Query::Number(n) => {
                return Ok(find(boards, now, |c| {
                    c.get("idShort").and_then(Value::as_i64) == Some(n)
                }));
            }
            Query::Link(link) => {
                let hits = find(boards, now, |c| nested_string(c, &["shortLink"]) == link);
                if !hits.is_empty() || q.contains("trello.com/c/") {
                    return Ok(hits);
                }
                link
            }
            Query::Text(text) => text,
        };
        let ids: Vec<String> = boards.iter().map(|b| nested_string(b, &["id"])).collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let found = self
            .client
            .call(
                Method::GET,
                "/search",
                json!({
                    "query": text, "modelTypes": "cards", "idBoards": ids.join(","),
                    "cards_limit": 20, "partial": true, "card_fields": CARD_FIELDS,
                }),
                None,
            )
            .await?;
        let by_board: HashMap<String, &Value> = boards
            .iter()
            .map(|b| (nested_string(b, &["id"]), b))
            .collect();
        Ok(object_slice(found.get("cards").unwrap_or(&Value::Null))
            .into_iter()
            .filter_map(|c| {
                let c = Value::Object(c);
                by_board
                    .get(&nested_string(&c, &["idBoard"]))
                    .map(|b| card_view(&c, b, now))
            })
            .collect())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::mcp::test_support::{calls, json_response, not_found, query, test_server};

    pub(crate) const NOW_ISO: &str = "2026-10-16T12:00:00.000Z";

    fn now() -> i64 {
        unix_seconds(NOW_ISO).unwrap()
    }

    fn at(seconds_ago: i64) -> String {
        iso_date(now() - seconds_ago)
    }

    fn card(id: &str, number: i64, name: &str, link: &str, list: &str, pos: f64) -> Value {
        json!({
            "id": id, "idShort": number, "name": name, "shortUrl": format!("https://trello.com/c/{link}"),
            "shortLink": link, "idList": list, "idBoard": "b1", "idMembers": ["m1"], "pos": pos,
            "dateLastActivity": at(2 * 3600),
        })
    }

    /// One board as `board_snapshot` returns it.
    pub(crate) fn board() -> Value {
        json!({
            "id": "b1", "name": "Client Portal", "url": "https://trello.com/b/B1/client-portal",
            "lists": [{"id": "l1", "name": "In Progress"}, {"id": "l2", "name": "Ready for Dev 🚀"}, {"id": "l3", "name": "Done"}],
            "members": [{"id": "m1", "initials": "MT", "fullName": "Mai Tran"}],
            "cards": [
                card("c1", 348, "Block physical address", "aaaa1111", "l1", 1.0),
                card("c2", 61, "Migrate cron jobs", "bbbb2222", "l2", 2.0),
                card("c3", 7, "Rate limit quotes", "cccc3333", "l2", 1.0),
                card("c4", 9, "Shipped thing", "dddd4444", "l3", 1.0),
            ]
        })
    }

    fn mention(id: &str, card: &str, seconds_ago: i64, text: &str) -> Value {
        json!({
            "id": format!("n-{id}"), "type": "mentionedOnCard", "date": at(seconds_ago), "idAction": id,
            "data": {"text": text, "card": {"id": card}, "board": {"id": "b1"}},
            "memberCreator": {"fullName": "Mai Tran"},
        })
    }

    fn my_comment(card: &str, seconds_ago: i64) -> Value {
        json!({"date": at(seconds_ago), "data": {"card": {"id": card}}})
    }

    #[test]
    fn list_names_match_loosely() {
        for (name, kind) in [
            ("Ready for dev", Some(ListKind::ReadyForDev)),
            ("Ready for Development", Some(ListKind::ReadyForDev)),
            ("READY FOR DEV 🚀", Some(ListKind::ReadyForDev)),
            ("In progress", Some(ListKind::InProgress)),
            ("In-Progress", Some(ListKind::InProgress)),
            ("In progress (BE)", Some(ListKind::InProgress)),
            ("Ready", None),
            ("Not in progress", None),
            ("Progress review", None),
            ("", None),
        ] {
            assert_eq!(list_kind(name), kind, "{name}");
        }
    }

    #[test]
    fn trello_dates() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(unix_seconds("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(iso_date(951_868_800), "2000-03-01T00:00:00.000Z");
        let t = unix_seconds("2026-10-16T03:12:45.123Z").unwrap();
        assert_eq!(iso_date(t), "2026-10-16T03:12:45.000Z");
        assert_eq!(iso_date(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(unix_seconds("nope"), None);
        assert_eq!(unix_seconds(""), None);
    }

    #[test]
    fn named_lists_keep_list_and_card_order() {
        let boards = [board()];
        let ready = named_lists(&boards, ListKind::ReadyForDev, now());
        assert_eq!(ready.len(), 1);
        assert_eq!(
            (&ready[0]["board"], &ready[0]["url"]),
            (
                &json!("Client Portal"),
                &json!("https://trello.com/b/B1/client-portal")
            )
        );
        assert_eq!(
            ready[0]["cards"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["number"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            [7, 61]
        );
        assert_eq!(
            ready[0]["cards"][0],
            json!({"id": "c3", "number": 7, "name": "Rate limit quotes", "url": "https://trello.com/c/cccc3333",
                   "board": "Client Portal", "list": "Ready for Dev 🚀", "members": ["MT"], "activity": "2h ago"})
        );
        let progress = named_lists(&boards, ListKind::InProgress, now());
        assert_eq!(progress[0]["cards"][0]["number"], 348);
        let mut empty = board();
        empty["lists"] = json!([{"id": "l3", "name": "Done"}]);
        assert!(
            named_lists(&[empty], ListKind::InProgress, now()).is_empty(),
            "a board without such a list is left out"
        );
    }

    #[test]
    fn waiting_keeps_unanswered_mentions_only() {
        let notes = json!([
            mention(
                "a1",
                "c1",
                2 * 3600,
                "@loithai can you confirm the API returns 422?"
            ),
            mention("a2", "c2", 86_400, "@loithai is the retry policy final?"),
            mention("a3", "c2", 3 * 86_400, "@loithai ping"),
            mention("a4", "c3", 5 * 86_400, "@loithai answered already"),
            mention("a5", "c1", 40 * 86_400, "@loithai too old"),
            mention("a6", "gone", 3600, "@loithai on an archived card"),
            json!({"id": "n-x", "date": at(60), "data": {}}),
        ]);
        let mine = json!([my_comment("c3", 4 * 86_400), my_comment("c1", 3 * 3600)]);
        let rows = waiting_rows(&notes, &mine, &[board()], now());
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert_eq!(
            (
                &rows[0]["number"],
                &rows[0]["mention"],
                &rows[0]["by"],
                &rows[0]["age"],
                &rows[0]["earlier"]
            ),
            (
                &json!(348),
                &json!("@loithai can you confirm the API returns 422?"),
                &json!("Mai Tran"),
                &json!("2h ago"),
                &json!(0)
            )
        );
        assert_eq!(
            rows[0]["commentUrl"],
            "https://trello.com/c/aaaa1111#comment-a1"
        );
        assert_eq!(rows[0]["list"], "In Progress");
        assert_eq!(
            (&rows[1]["number"], &rows[1]["earlier"], &rows[1]["age"]),
            (&json!(61), &json!(1), &json!("1d ago"))
        );
        assert_eq!(
            rows[1]["commentUrl"], "https://trello.com/c/bbbb2222#comment-a2",
            "the latest mention"
        );
    }

    #[test]
    fn long_mentions_are_cut_on_a_character() {
        let text = "é😀 Xin chào ".repeat(40);
        let rows = waiting_rows(
            &json!([mention("a1", "c1", 60, &text)]),
            &json!([]),
            &[board()],
            now(),
        );
        let cut = rows[0]["mention"].as_str().unwrap();
        assert_eq!(cut.chars().count(), 280);
        assert!(cut.ends_with('…') && text.starts_with(cut.trim_end_matches('…')));
        let mut no_action = mention("", "c1", 60, "hi");
        no_action.as_object_mut().unwrap().remove("idAction");
        let rows = waiting_rows(&json!([no_action]), &json!([]), &[board()], now());
        assert_eq!(
            rows[0]["commentUrl"], "https://trello.com/c/aaaa1111",
            "no action id: the card's link"
        );
    }

    #[test]
    fn queries_are_classified() {
        assert!(matches!(classify("  ").unwrap(), Query::Empty));
        assert!(matches!(classify("#348").unwrap(), Query::Number(348)));
        assert!(matches!(classify("348").unwrap(), Query::Number(348)));
        assert!(
            matches!(classify("https://trello.com/c/kfp0Te5b/348-block-physical").unwrap(), Query::Link(l) if l == "kfp0Te5b")
        );
        assert!(matches!(classify("kfp0Te5b").unwrap(), Query::Link(l) if l == "kfp0Te5b"));
        assert!(
            matches!(classify("invoices").unwrap(), Query::Text(t) if t == "invoices"),
            "8 letters, no digit: words"
        );
        assert!(matches!(classify("invoice pdf").unwrap(), Query::Text(t) if t == "invoice pdf"));
        assert!(classify(&"a".repeat(200)).is_ok() && classify(&"a".repeat(201)).is_err());
    }

    #[tokio::test]
    async fn snapshot_and_waiting_ask_trello_for_the_right_things() {
        let h = test_server(&[], |req| match req.url.path() {
            "/boards/b1" => json_response(board()),
            "/members/me/notifications" => {
                json_response(json!([mention("a1", "c1", 60, "@me hi")]))
            }
            "/members/me/actions" => json_response(json!([])),
            _ => not_found(),
        })
        .await;
        let boards = vec![h.server.board_snapshot("b1").await.unwrap()];
        let rows = h.server.waiting(&boards, now()).await.unwrap();
        assert_eq!(rows.len(), 1);
        let reqs = h.api.received_requests().await.unwrap();
        let q = |path: &str| query(reqs.iter().find(|r| r.url.path() == path).unwrap());
        let b = q("/boards/b1");
        assert_eq!(
            (
                b["lists"].as_str(),
                b["cards"].as_str(),
                b["members"].as_str()
            ),
            ("open", "open", "all")
        );
        assert!(
            b["card_fields"].contains("shortLink")
                && b["card_fields"].contains("idShort")
                && b["card_fields"].contains("pos")
        );
        assert_eq!(q("/members/me/notifications")["filter"], "mentionedOnCard");
        let a = q("/members/me/actions");
        assert_eq!(
            (a["filter"].as_str(), a["since"].as_str()),
            ("commentCard", iso_date(now() - 30 * 86_400).as_str())
        );
    }

    #[tokio::test]
    async fn search_uses_the_snapshot_for_numbers_and_links() {
        let h = test_server(&[], |req| match req.url.path() {
            "/search" => json_response(json!({"cards": [card("c1", 348, "Block physical address", "aaaa1111", "l1", 1.0)]})),
            _ => not_found(),
        })
        .await;
        let boards = [board()];
        let s = &h.server;
        assert_eq!(
            s.search_cards("#61", &boards, now()).await.unwrap()[0]["name"],
            "Migrate cron jobs"
        );
        let url = "https://trello.com/c/cccc3333/7-rate-limit";
        assert_eq!(
            s.search_cards(url, &boards, now()).await.unwrap()[0]["number"],
            7
        );
        assert!(
            s.search_cards("https://trello.com/c/zzzz9999", &boards, now())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(s.search_cards("", &boards, now()).await.unwrap().is_empty());
        assert!(
            calls(&h.api).await.is_empty(),
            "numbers, links and empty queries never call Trello"
        );
        let found = s.search_cards("address", &boards, now()).await.unwrap();
        assert_eq!(
            (&found[0]["number"], &found[0]["list"]),
            (&json!(348), &json!("In Progress"))
        );
        let reqs = h.api.received_requests().await.unwrap();
        let q = query(&reqs[0]);
        assert_eq!(
            (
                q["query"].as_str(),
                q["modelTypes"].as_str(),
                q["idBoards"].as_str()
            ),
            ("address", "cards", "b1")
        );
        // A bare 8-character short link that is no open card falls back to words.
        s.search_cards("abcd1234", &boards, now()).await.unwrap();
        assert_eq!(calls(&h.api).await.len(), 2);
    }
}
