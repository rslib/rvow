//! Immutable ids of the form `KIND-xxxxxx`: an uppercase kind and 6 lowercase
//! Crockford base32 characters. An id is never changed and never reused.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use crate::error::Error;

pub const BODY_LEN: usize = 6;

const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Req,
    Tsk,
    Chg,
    Amd,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Req, Kind::Tsk, Kind::Chg, Kind::Amd];

    pub fn prefix(self) -> &'static str {
        match self {
            Kind::Req => "REQ",
            Kind::Tsk => "TSK",
            Kind::Chg => "CHG",
            Kind::Amd => "AMD",
        }
    }

    fn from_prefix(s: &str) -> Option<Kind> {
        Kind::ALL
            .into_iter()
            .find(|k| k.prefix().eq_ignore_ascii_case(s))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id {
    kind: Kind,
    body: String,
}

/// A reference written in a file: a real id or a placeholder such as `REQ-new1`
/// that the next command replaces with a minted id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ref {
    Id(Id),
    Placeholder { kind: Kind, n: u32 },
}

impl Ref {
    pub fn kind(&self) -> Kind {
        match self {
            Ref::Id(id) => id.kind,
            Ref::Placeholder { kind, .. } => *kind,
        }
    }
}

impl fmt::Display for Ref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ref::Id(id) => id.fmt(f),
            Ref::Placeholder { kind, n } => write!(f, "{}-new{n}", kind.prefix()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    MissingKind,
    UnknownKind(String),
    BadLength(usize),
    BadChar(char),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::MissingKind => write!(f, "id needs a kind, as in REQ-k3f9m2"),
            ParseError::UnknownKind(k) => {
                write!(f, "unknown id kind {k:?}, expected REQ, TSK, CHG or AMD")
            }
            ParseError::BadLength(n) => {
                write!(f, "id body has {n} characters, expected {BODY_LEN}")
            }
            ParseError::BadChar(c) => write!(f, "{c:?} is not a Crockford base32 character"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Maps one input character to its canonical Crockford form, accepting any case
/// and reading `i` and `l` as `1` and `o` as `0`.
fn normalize_char(c: char) -> Result<char, ParseError> {
    let c = c.to_ascii_lowercase();
    let c = match c {
        'i' | 'l' => '1',
        'o' => '0',
        c => c,
    };
    if c.is_ascii() && ALPHABET.contains(&(c as u8)) {
        Ok(c)
    } else {
        Err(ParseError::BadChar(c))
    }
}

fn normalize_body(s: &str) -> Result<String, ParseError> {
    s.chars().map(normalize_char).collect()
}

impl Id {
    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    /// Draws a fresh id of `kind` that is not in `taken`.
    ///
    /// Fails only when the operating system has no random source.
    pub fn mint(kind: Kind, taken: &HashSet<Id>) -> crate::error::Result<Id> {
        loop {
            let bytes = Error::random::<BODY_LEN>()?;
            let body = bytes
                .iter()
                .map(|b| ALPHABET[(b & 31) as usize] as char)
                .collect();
            let id = Id { kind, body };
            if !taken.contains(&id) {
                return Ok(id);
            }
        }
    }
}

impl FromStr for Id {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Id, ParseError> {
        let (kind, body) = s.split_once('-').ok_or(ParseError::MissingKind)?;
        let kind = Kind::from_prefix(kind).ok_or_else(|| ParseError::UnknownKind(kind.into()))?;
        let len = body.chars().count();
        if len != BODY_LEN {
            return Err(ParseError::BadLength(len));
        }
        Ok(Id {
            kind,
            body: normalize_body(body)?,
        })
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.kind.prefix(), self.body)
    }
}

impl FromStr for Ref {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Ref, ParseError> {
        if let Some((kind, rest)) = s.split_once('-')
            && let Some(n) = rest.strip_prefix("new")
            && let Ok(n) = n.parse()
        {
            let kind =
                Kind::from_prefix(kind).ok_or_else(|| ParseError::UnknownKind(kind.into()))?;
            return Ok(Ref::Placeholder { kind, n });
        }
        s.parse().map(Ref::Id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    Invalid(ParseError),
    NotFound,
    Ambiguous(Vec<Id>),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::Invalid(e) => e.fmt(f),
            ResolveError::NotFound => write!(f, "no id matches"),
            ResolveError::Ambiguous(ids) => {
                let ids: Vec<String> = ids.iter().map(Id::to_string).collect();
                write!(f, "several ids match: {}", ids.join(", "))
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// Finds the one id among `known` that a user's short form names, as git does
/// with commit prefixes. `x7k`, `TSK-x7k` and `tsk-X7K` all name `TSK-x7k2q4`.
pub fn resolve<'a>(
    input: &str,
    known: impl IntoIterator<Item = &'a Id>,
) -> Result<Id, ResolveError> {
    let (kind, body) = match input.split_once('-') {
        Some((k, b)) => (
            Some(
                Kind::from_prefix(k)
                    .ok_or_else(|| ResolveError::Invalid(ParseError::UnknownKind(k.into())))?,
            ),
            b,
        ),
        None => (None, input),
    };
    let body = normalize_body(body).map_err(ResolveError::Invalid)?;
    if body.is_empty() {
        return Err(ResolveError::NotFound);
    }
    let mut hits: Vec<Id> = known
        .into_iter()
        .filter(|id| kind.is_none_or(|k| id.kind == k) && id.body.starts_with(&body))
        .cloned()
        .collect();
    hits.sort();
    hits.dedup();
    match hits.len() {
        0 => Err(ResolveError::NotFound),
        1 => Ok(hits.remove(0)),
        _ => Err(ResolveError::Ambiguous(hits)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Id {
        s.parse().unwrap()
    }

    #[test]
    fn parse_and_display_round_trip() {
        assert_eq!(id("REQ-k3f9m2").to_string(), "REQ-k3f9m2");
        assert_eq!(id("req-K3F9M2").to_string(), "REQ-k3f9m2");
    }

    #[test]
    fn ambiguous_letters_map_to_digits() {
        assert_eq!(id("TSK-iLoabc"), id("TSK-110abc"));
    }

    #[test]
    fn rejects_bad_ids() {
        assert_eq!("k3f9m2".parse::<Id>(), Err(ParseError::MissingKind));
        assert_eq!(
            "FOO-k3f9m2".parse::<Id>(),
            Err(ParseError::UnknownKind("FOO".into()))
        );
        assert_eq!("REQ-k3f9m".parse::<Id>(), Err(ParseError::BadLength(5)));
        assert_eq!("REQ-k3f9mu".parse::<Id>(), Err(ParseError::BadChar('u')));
    }

    #[test]
    fn placeholders_parse_apart_from_ids() {
        assert_eq!(
            "REQ-new1".parse::<Ref>(),
            Ok(Ref::Placeholder {
                kind: Kind::Req,
                n: 1
            })
        );
        assert_eq!("TSK-x7k2q4".parse::<Ref>(), Ok(Ref::Id(id("TSK-x7k2q4"))));
        assert!("REQ-newx".parse::<Ref>().is_err());
    }

    #[test]
    fn mint_has_the_right_form_and_avoids_taken_ids() {
        let mut taken = HashSet::new();
        for _ in 0..2000 {
            let new = Id::mint(Kind::Chg, &taken).unwrap();
            assert_eq!(new.to_string().parse::<Id>().unwrap(), new);
            assert!(taken.insert(new));
        }
    }

    #[test]
    fn resolve_by_prefix() {
        let known = [id("TSK-x7k2q4"), id("TSK-x7m000"), id("REQ-x7k999")];
        assert_eq!(resolve("x7k2", &known), Ok(id("TSK-x7k2q4")));
        assert_eq!(resolve("TSK-x7k", &known), Ok(id("TSK-x7k2q4")));
        assert_eq!(resolve("tsk-X7K", &known), Ok(id("TSK-x7k2q4")));
        assert_eq!(
            resolve("x7k", &known),
            Err(ResolveError::Ambiguous(vec![
                id("REQ-x7k999"),
                id("TSK-x7k2q4")
            ]))
        );
        assert_eq!(resolve("zzz", &known), Err(ResolveError::NotFound));
        assert_eq!(resolve("", &known), Err(ResolveError::NotFound));
    }
}
