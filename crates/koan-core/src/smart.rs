//! Smart playlists: a playlist whose contents are a rule over the library
//! rather than a list someone made.
//!
//! The rule is stored on the playlist row as JSON in the shape [`Rules`]
//! serialises to, and compiled to SQL by `db::queries::smart`. What a rule
//! selects is written into the playlist's entries like any other playlist's,
//! so every reader (counts, covers, the queue following a playlist, Subsonic,
//! the apps) sees an ordinary playlist that it may not edit.
//!
//! The JSON shape, as callers write it:
//!
//! ```json
//! {
//!   "match": "all",
//!   "rules": [
//!     { "field": "playCount", "op": "gt", "value": 5 },
//!     { "match": "any", "rules": [
//!       { "field": "genre", "op": "contains", "value": "jazz" },
//!       { "field": "favourite", "op": "is", "value": true }
//!     ] }
//!   ],
//!   "sort": [{ "field": "lastPlayed", "desc": true }],
//!   "limit": 100
//! }
//! ```
//!
//! Navidrome's `.nsp` files are read into the same model by [`from_nsp`].

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How deep groups may nest, and how many conditions a rule may hold in all.
/// Rules arrive from files and from the API; neither should be able to build
/// a query SQLite refuses or takes a long time to plan.
const MAX_DEPTH: usize = 8;
const MAX_CONDITIONS: usize = 200;

/// A smart playlist's definition: which tracks, in what order, how many.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    #[serde(rename = "match", default)]
    pub matching: Match,
    #[serde(default)]
    pub rules: Vec<Condition>,
    /// Applied in order. Ties fall back to album order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<Sort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Whether every condition in a group must hold, or any one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Match {
    #[default]
    All,
    Any,
}

/// A condition, or a nested group of them. Told apart by shape: a group has
/// `rules`, a condition has `field`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Value", into = "Value")]
pub enum Condition {
    Group {
        matching: Match,
        rules: Vec<Condition>,
    },
    Rule(Rule),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub field: Field,
    pub op: Op,
    pub value: Value,
}

/// What a condition or a sort can look at. Per-account fields (`playCount`,
/// `lastPlayed`, `favourite`) are the playlist owner's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Field {
    Title,
    Artist,
    AlbumArtist,
    Album,
    Genre,
    /// The codec, as koan names it: `FLAC`, `MP3`, `AAC`…
    Format,
    /// The file's path on disk. Absent for tracks only on a server.
    Path,
    Year,
    /// Seconds.
    Duration,
    BitDepth,
    SampleRate,
    TrackNumber,
    DiscNumber,
    PlayCount,
    LastPlayed,
    /// When the track's album entered the library.
    DateAdded,
    Favourite,
}

/// How a field is compared, which decides the operators and values it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Number,
    Date,
    Bool,
}

impl Field {
    pub const ALL: [Field; 17] = [
        Field::Title,
        Field::Artist,
        Field::AlbumArtist,
        Field::Album,
        Field::Genre,
        Field::Format,
        Field::Path,
        Field::Year,
        Field::Duration,
        Field::BitDepth,
        Field::SampleRate,
        Field::TrackNumber,
        Field::DiscNumber,
        Field::PlayCount,
        Field::LastPlayed,
        Field::DateAdded,
        Field::Favourite,
    ];

    pub fn kind(self) -> Kind {
        match self {
            Field::Title
            | Field::Artist
            | Field::AlbumArtist
            | Field::Album
            | Field::Genre
            | Field::Format
            | Field::Path => Kind::Text,
            Field::Year
            | Field::Duration
            | Field::BitDepth
            | Field::SampleRate
            | Field::TrackNumber
            | Field::DiscNumber
            | Field::PlayCount => Kind::Number,
            Field::LastPlayed | Field::DateAdded => Kind::Date,
            Field::Favourite => Kind::Bool,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Artist => "artist",
            Field::AlbumArtist => "albumArtist",
            Field::Album => "album",
            Field::Genre => "genre",
            Field::Format => "format",
            Field::Path => "path",
            Field::Year => "year",
            Field::Duration => "duration",
            Field::BitDepth => "bitDepth",
            Field::SampleRate => "sampleRate",
            Field::TrackNumber => "trackNumber",
            Field::DiscNumber => "discNumber",
            Field::PlayCount => "playCount",
            Field::LastPlayed => "lastPlayed",
            Field::DateAdded => "dateAdded",
            Field::Favourite => "favourite",
        }
    }

    fn parse(name: &str) -> Result<Self, String> {
        Field::ALL
            .into_iter()
            .find(|f| f.name().eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                let known: Vec<&str> = Field::ALL.iter().map(|f| f.name()).collect();
                format!("unknown field '{name}'; known fields: {}", known.join(", "))
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Op {
    Is,
    IsNot,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    Gt,
    Lt,
    /// Inclusive, `[low, high]`.
    InTheRange,
    /// Dates, `YYYY-MM-DD`.
    Before,
    After,
    /// A number of days back from now.
    InTheLast,
    /// Not within that many days, or never.
    NotInTheLast,
}

impl Op {
    const ALL: [Op; 13] = [
        Op::Is,
        Op::IsNot,
        Op::Contains,
        Op::NotContains,
        Op::StartsWith,
        Op::EndsWith,
        Op::Gt,
        Op::Lt,
        Op::InTheRange,
        Op::Before,
        Op::After,
        Op::InTheLast,
        Op::NotInTheLast,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Op::Is => "is",
            Op::IsNot => "isNot",
            Op::Contains => "contains",
            Op::NotContains => "notContains",
            Op::StartsWith => "startsWith",
            Op::EndsWith => "endsWith",
            Op::Gt => "gt",
            Op::Lt => "lt",
            Op::InTheRange => "inTheRange",
            Op::Before => "before",
            Op::After => "after",
            Op::InTheLast => "inTheLast",
            Op::NotInTheLast => "notInTheLast",
        }
    }

    fn parse(name: &str) -> Result<Self, String> {
        Op::ALL
            .into_iter()
            .find(|o| o.name().eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                let known: Vec<&str> = Op::ALL.iter().map(|o| o.name()).collect();
                format!(
                    "unknown operator '{name}'; known operators: {}",
                    known.join(", ")
                )
            })
    }

    /// Which operators each kind of field takes.
    pub fn applies_to(self, kind: Kind) -> bool {
        use Op::*;
        match kind {
            Kind::Text => matches!(
                self,
                Is | IsNot | Contains | NotContains | StartsWith | EndsWith
            ),
            Kind::Number => matches!(self, Is | IsNot | Gt | Lt | InTheRange),
            Kind::Date => matches!(self, Before | After | InTheLast | NotInTheLast | InTheRange),
            Kind::Bool => matches!(self, Is | IsNot),
        }
    }
}

/// One sort key: a field, or `random`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Value", into = "Value")]
pub struct Sort {
    /// `None` is a random order, drawn again each time the playlist is.
    pub field: Option<Field>,
    pub desc: bool,
}

impl TryFrom<Value> for Condition {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, String> {
        let Value::Object(map) = &value else {
            return Err(format!("a condition must be an object, not {value}"));
        };
        if map.contains_key("rules") {
            let matching = match map.get("match") {
                None => Match::All,
                Some(m) => serde_json::from_value(m.clone())
                    .map_err(|_| format!("'match' must be \"all\" or \"any\", not {m}"))?,
            };
            let rules = serde_json::from_value(map["rules"].clone()).map_err(|e| e.to_string())?;
            return Ok(Condition::Group { matching, rules });
        }
        let text = |key: &str| {
            map.get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("a condition needs '{key}' as a string: {value}"))
        };
        let field = Field::parse(text("field")?)?;
        let op = Op::parse(text("op")?)?;
        let value = map
            .get("value")
            .cloned()
            .ok_or_else(|| format!("a condition needs a 'value': {value}"))?;
        Ok(Condition::Rule(Rule { field, op, value }))
    }
}

impl From<Condition> for Value {
    fn from(c: Condition) -> Value {
        match c {
            Condition::Group { matching, rules } => {
                serde_json::json!({ "match": matching, "rules": rules })
            }
            Condition::Rule(r) => {
                serde_json::json!({ "field": r.field, "op": r.op, "value": r.value })
            }
        }
    }
}

impl TryFrom<Value> for Sort {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, String> {
        let (name, desc) = match &value {
            Value::String(s) => (s.as_str(), false),
            Value::Object(map) => (
                map.get("field")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("a sort needs 'field' as a string: {value}"))?,
                map.get("desc").and_then(Value::as_bool).unwrap_or(false),
            ),
            _ => return Err(format!("a sort must be an object or a field name: {value}")),
        };
        if name.eq_ignore_ascii_case("random") {
            return Ok(Sort { field: None, desc });
        }
        Ok(Sort {
            field: Some(Field::parse(name)?),
            desc,
        })
    }
}

impl From<Sort> for Value {
    fn from(s: Sort) -> Value {
        let field = s.field.map_or("random", Field::name);
        serde_json::json!({ "field": field, "desc": s.desc })
    }
}

impl Rules {
    /// Read rules from their JSON, checking every condition: the error names
    /// what is wrong, for a caller writing rules by hand.
    pub fn parse(json: &str) -> Result<Self, String> {
        let rules: Rules = serde_json::from_str(json).map_err(|e| e.to_string())?;
        rules.check()?;
        Ok(rules)
    }

    pub fn from_value(value: Value) -> Result<Self, String> {
        let rules: Rules = serde_json::from_value(value).map_err(|e| e.to_string())?;
        rules.check()?;
        Ok(rules)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("rules serialise")
    }

    /// Whether the order is drawn at random, so each evaluation differs.
    pub fn is_random(&self) -> bool {
        self.sort.iter().any(|s| s.field.is_none())
    }

    /// Every condition takes an operator its field allows, with a value of
    /// the right type; nesting and size stay within bounds.
    pub fn check(&self) -> Result<(), String> {
        let mut count = 0;
        check_group(&self.rules, 1, &mut count)
    }
}

fn check_group(rules: &[Condition], depth: usize, count: &mut usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("groups nest more than {MAX_DEPTH} deep"));
    }
    for condition in rules {
        *count += 1;
        if *count > MAX_CONDITIONS {
            return Err(format!("more than {MAX_CONDITIONS} conditions"));
        }
        match condition {
            Condition::Group { rules, .. } => check_group(rules, depth + 1, count)?,
            Condition::Rule(rule) => {
                rule.operand()?;
            }
        }
    }
    Ok(())
}

/// A condition's value, checked against its field and operator.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Text(String),
    Number(f64),
    NumberRange(f64, f64),
    /// Unix seconds.
    Instant(i64),
    /// Unix seconds, inclusive at both ends.
    InstantRange(i64, i64),
    Days(f64),
    Bool(bool),
}

impl Rule {
    pub fn operand(&self) -> Result<Operand, String> {
        let kind = self.field.kind();
        let (field, op) = (self.field.name(), self.op.name());
        if !self.op.applies_to(kind) {
            let allowed: Vec<&str> = Op::ALL
                .iter()
                .filter(|o| o.applies_to(kind))
                .map(|o| o.name())
                .collect();
            return Err(format!(
                "'{field}' does not take '{op}'; it takes {}",
                allowed.join(", ")
            ));
        }
        let wrong = |wanted: &str| format!("'{field} {op}' wants {wanted}, not {}", self.value);
        let pair = || match &self.value {
            Value::Array(items) if items.len() == 2 => Ok((&items[0], &items[1])),
            _ => Err(wrong("a pair, [low, high]")),
        };
        Ok(match (kind, self.op) {
            (Kind::Text, _) => Operand::Text(
                self.value
                    .as_str()
                    .ok_or_else(|| wrong("a string"))?
                    .to_owned(),
            ),
            (Kind::Bool, _) => {
                Operand::Bool(self.value.as_bool().ok_or_else(|| wrong("true or false"))?)
            }
            (Kind::Number, Op::InTheRange) => {
                let (lo, hi) = pair()?;
                let num = |v: &Value| v.as_f64().ok_or_else(|| wrong("two numbers"));
                Operand::NumberRange(num(lo)?, num(hi)?)
            }
            (Kind::Number, _) => {
                Operand::Number(self.value.as_f64().ok_or_else(|| wrong("a number"))?)
            }
            (Kind::Date, Op::InTheLast | Op::NotInTheLast) => {
                let days = self
                    .value
                    .as_f64()
                    .ok_or_else(|| wrong("a number of days"))?;
                if days < 0.0 {
                    return Err(wrong("a positive number of days"));
                }
                Operand::Days(days)
            }
            (Kind::Date, Op::InTheRange) => {
                let (lo, hi) = pair()?;
                let day = |v: &Value| {
                    v.as_str()
                        .and_then(parse_day)
                        .ok_or_else(|| wrong("two dates, YYYY-MM-DD"))
                };
                Operand::InstantRange(day(lo)?, day(hi)? + 86_399)
            }
            (Kind::Date, _) => Operand::Instant(
                self.value
                    .as_str()
                    .and_then(parse_day)
                    .ok_or_else(|| wrong("a date, YYYY-MM-DD"))?,
            ),
        })
    }
}

/// Midnight UTC at the start of a `YYYY-MM-DD` day, as unix seconds.
fn parse_day(s: &str) -> Option<i64> {
    let day = chrono::NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").ok()?;
    Some(day.and_hms_opt(0, 0, 0)?.and_utc().timestamp())
}

/// A Navidrome smart playlist file, read into kōan's model.
#[derive(Debug, Clone, PartialEq)]
pub struct Nsp {
    pub name: Option<String>,
    pub comment: Option<String>,
    pub rules: Rules,
}

/// Read a Navidrome `.nsp` file.
///
/// Navidrome writes `{"all": [...]}` or `{"any": [...]}` with each condition as
/// `{"<operator>": {"<field>": <value>}}`, plus `sort`, `order` and `limit`.
/// Field names are matched case-insensitively, as Navidrome does. A field or
/// operator kōan has no counterpart for (ratings, BPM, comments, other
/// playlists) is an error naming it: a playlist that silently dropped a
/// condition would select more than its author asked for.
pub fn from_nsp(json: &str) -> Result<Nsp, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
    let Value::Object(map) = &root else {
        return Err("not a JSON object".into());
    };
    let text = |key: &str| map.get(key).and_then(Value::as_str).map(str::to_owned);
    let (matching, rules) = nsp_group(&root)?.ok_or("neither 'all' nor 'any' at the top")?;

    let desc = text("order").is_some_and(|o| o.eq_ignore_ascii_case("desc"));
    let mut sort = Vec::new();
    if let Some(keys) = text("sort") {
        // Newer Navidrome takes several keys, comma separated, each with an
        // optional sign; `order` applies to keys without one.
        for key in keys.split(',').map(str::trim).filter(|k| !k.is_empty()) {
            let (key, desc) = match key.as_bytes()[0] {
                b'-' => (&key[1..], true),
                b'+' => (&key[1..], false),
                _ => (key, desc),
            };
            if key.eq_ignore_ascii_case("random") {
                sort.push(Sort { field: None, desc });
            } else {
                sort.push(Sort {
                    field: Some(nsp_field(key)?),
                    desc,
                });
            }
        }
    }
    let limit = match map.get("limit") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("'limit' must be a whole number, not {v}"))?,
        ),
    };
    let rules = Rules {
        matching,
        rules,
        sort,
        limit: limit.filter(|&n| n > 0),
    };
    rules.check()?;
    Ok(Nsp {
        name: text("name").filter(|n| !n.trim().is_empty()),
        comment: text("comment").filter(|c| !c.trim().is_empty()),
        rules,
    })
}

/// `{"all": [...]}` or `{"any": [...]}`, if the object is a group.
fn nsp_group(value: &Value) -> Result<Option<(Match, Vec<Condition>)>, String> {
    for (key, matching) in [("all", Match::All), ("any", Match::Any)] {
        if let Some(list) = value.get(key) {
            let list = list
                .as_array()
                .ok_or_else(|| format!("'{key}' must be a list"))?;
            let rules = list.iter().map(nsp_condition).collect::<Result<_, _>>()?;
            return Ok(Some((matching, rules)));
        }
    }
    Ok(None)
}

fn nsp_condition(value: &Value) -> Result<Condition, String> {
    if let Some((matching, rules)) = nsp_group(value)? {
        return Ok(Condition::Group { matching, rules });
    }
    let single = |v: &Value| -> Option<(String, Value)> {
        let map = v.as_object()?;
        (map.len() == 1).then(|| map.iter().next().map(|(k, v)| (k.clone(), v.clone())))?
    };
    let (op_name, inner) = single(value).ok_or_else(|| format!("not a condition: {value}"))?;
    let (field_name, raw) =
        single(&inner).ok_or_else(|| format!("'{op_name}' wants one field: {inner}"))?;
    let op = match op_name.to_ascii_lowercase().as_str() {
        "inplaylist" | "notinplaylist" => {
            return Err(format!("'{op_name}' (other playlists) is not supported"));
        }
        _ => Op::parse(&op_name)?,
    };
    let field = nsp_field(&field_name)?;
    let value = nsp_value(field.kind(), op, raw);
    let rule = Rule { field, op, value };
    rule.operand()?;
    Ok(Condition::Rule(rule))
}

/// Navidrome's field names, lowercased, and what they are in kōan.
fn nsp_field(name: &str) -> Result<Field, String> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "title" => Field::Title,
        "artist" => Field::Artist,
        "albumartist" => Field::AlbumArtist,
        "album" => Field::Album,
        "genre" => Field::Genre,
        "filetype" => Field::Format,
        "filepath" => Field::Path,
        "year" => Field::Year,
        "duration" => Field::Duration,
        "bitdepth" => Field::BitDepth,
        "samplerate" => Field::SampleRate,
        "tracknumber" => Field::TrackNumber,
        "discnumber" => Field::DiscNumber,
        "playcount" => Field::PlayCount,
        "lastplayed" => Field::LastPlayed,
        "dateadded" => Field::DateAdded,
        "loved" => Field::Favourite,
        _ => return Err(format!("field '{name}' is not supported")),
    })
}

/// Navidrome files are hand-written, and numbers and booleans turn up as
/// strings. They are taken as what the field wants where they read as one.
fn nsp_value(kind: Kind, op: Op, raw: Value) -> Value {
    let number = |v: Value| match &v {
        Value::String(s) => s
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or(v, Value::Number),
        _ => v,
    };
    match (kind, op, raw) {
        (Kind::Number, Op::InTheRange, Value::Array(items)) => {
            Value::Array(items.into_iter().map(number).collect())
        }
        (Kind::Number, _, v) | (Kind::Date, Op::InTheLast | Op::NotInTheLast, v) => number(v),
        (Kind::Bool, _, Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => Value::String(s),
        },
        (_, _, v) => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_round_trip_through_json() {
        let json = r#"{
            "match": "all",
            "rules": [
                { "field": "playCount", "op": "gt", "value": 5 },
                { "match": "any", "rules": [
                    { "field": "genre", "op": "contains", "value": "jazz" },
                    { "field": "favourite", "op": "is", "value": true }
                ] }
            ],
            "sort": [{ "field": "lastPlayed", "desc": true }, "random"],
            "limit": 100
        }"#;
        let rules = Rules::parse(json).unwrap();
        assert_eq!(rules.rules.len(), 2);
        assert_eq!(rules.sort[0].field, Some(Field::LastPlayed));
        assert!(rules.is_random());
        assert_eq!(Rules::parse(&rules.to_json()).unwrap(), rules);
    }

    #[test]
    fn errors_name_what_is_wrong() {
        let err = |json: &str| Rules::parse(json).unwrap_err();
        assert!(
            err(r#"{"rules":[{"field":"plays","op":"gt","value":1}]}"#)
                .contains("unknown field 'plays'")
        );
        assert!(
            err(r#"{"rules":[{"field":"title","op":"gt","value":1}]}"#)
                .contains("does not take 'gt'")
        );
        assert!(
            err(r#"{"rules":[{"field":"year","op":"is","value":"x"}]}"#).contains("wants a number")
        );
        assert!(
            err(r#"{"rules":[{"field":"lastPlayed","op":"before","value":"soon"}]}"#)
                .contains("YYYY-MM-DD")
        );
        assert!(
            err(r#"{"rules":[{"field":"year","op":"inTheRange","value":[1990]}]}"#)
                .contains("pair")
        );
    }

    #[test]
    fn nesting_is_bounded() {
        let mut json = r#"{"field":"year","op":"gt","value":1}"#.to_string();
        for _ in 0..=MAX_DEPTH {
            json = format!(r#"{{"rules":[{json}]}}"#);
        }
        assert!(Rules::parse(&json).unwrap_err().contains("nest"));
    }

    #[test]
    fn a_navidrome_file_reads_into_the_model() {
        let nsp = from_nsp(
            r#"{
                "name": "80's Favourites",
                "comment": "Loved, from the eighties",
                "all": [
                    { "any": [ {"is": {"loved": true}}, {"gt": {"playCount": "10"}} ] },
                    { "inTheRange": { "year": [1981, 1990] } },
                    { "inTheLast": { "lastPlayed": 30 } },
                    { "contains": { "Genre": "synth" } }
                ],
                "sort": "-playcount,title",
                "order": "asc",
                "limit": 25
            }"#,
        )
        .unwrap();
        assert_eq!(nsp.name.as_deref(), Some("80's Favourites"));
        let rules = nsp.rules;
        assert_eq!(rules.matching, Match::All);
        assert_eq!(rules.limit, Some(25));
        assert_eq!(
            rules.sort,
            vec![
                Sort {
                    field: Some(Field::PlayCount),
                    desc: true
                },
                Sort {
                    field: Some(Field::Title),
                    desc: false
                },
            ]
        );
        let Condition::Group {
            matching,
            rules: any,
        } = &rules.rules[0]
        else {
            panic!("expected a group")
        };
        assert_eq!(*matching, Match::Any);
        assert_eq!(
            any[1],
            Condition::Rule(Rule {
                field: Field::PlayCount,
                op: Op::Gt,
                value: serde_json::json!(10.0)
            }),
            "a number written as a string reads as a number"
        );
        assert!(matches!(
            &rules.rules[3],
            Condition::Rule(Rule {
                field: Field::Genre,
                op: Op::Contains,
                ..
            })
        ));
    }

    #[test]
    fn a_navidrome_file_with_an_unsupported_field_is_refused() {
        let err = from_nsp(r#"{"all":[{"gt":{"rating":3}}]}"#).unwrap_err();
        assert!(err.contains("rating"), "{err}");
        let err = from_nsp(r#"{"all":[{"inPlaylist":{"id":"x"}}]}"#).unwrap_err();
        assert!(err.contains("inPlaylist"), "{err}");
        assert!(from_nsp(r#"{"sort":"title"}"#).is_err());
    }

    #[test]
    fn random_sort_in_a_navidrome_file() {
        let nsp =
            from_nsp(r#"{"any":[{"is":{"loved":"true"}}],"sort":"random","limit":50}"#).unwrap();
        assert!(nsp.rules.is_random());
        assert_eq!(nsp.name, None);
    }
}
