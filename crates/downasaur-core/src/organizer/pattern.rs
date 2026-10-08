//! Directory pattern templates.
//!
//! Patterns are `/`-separated segments mixing literal text with tokens, e.g.
//! `{Platform}/{Author}/{Year}-{Month}`. The UI's drag-and-drop builder emits
//! the same tokens; `[Platform]` bracket syntax is accepted as an alias.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::naming::sanitize_component;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Token {
    Platform,
    Author,
    Year,
    Month,
    Day,
    Date,
    Resolution,
    MediaType,
}

impl Token {
    pub const ALL: [Self; 8] = [
        Self::Platform,
        Self::Author,
        Self::Year,
        Self::Month,
        Self::Day,
        Self::Date,
        Self::Resolution,
        Self::MediaType,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Platform => "Platform",
            Self::Author => "Author",
            Self::Year => "Year",
            Self::Month => "Month",
            Self::Day => "Day",
            Self::Date => "Date",
            Self::Resolution => "Resolution",
            Self::MediaType => "MediaType",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        let n = name.trim().to_ascii_lowercase();
        Some(match n.as_str() {
            "platform" | "platformname" => Self::Platform,
            "author" | "creator" | "channel" | "creatororchannelname" => Self::Author,
            "year" => Self::Year,
            "month" => Self::Month,
            "day" => Self::Day,
            "date" => Self::Date,
            "resolution" => Self::Resolution,
            "mediatype" | "type" => Self::MediaType,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum Part {
    Token(Token),
    Literal(String),
}

/// Values substituted into a pattern.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatternContext {
    pub platform: String,
    pub author: Option<String>,
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub resolution: Option<String>,
    pub media_type: String,
}

impl PatternContext {
    fn value(&self, t: Token) -> String {
        match t {
            Token::Platform => self.platform.clone(),
            Token::Author => self.author.clone().unwrap_or_else(|| "Unknown".into()),
            Token::Year => format!("{:04}", self.year),
            Token::Month => format!("{:02}", self.month),
            Token::Day => format!("{:02}", self.day),
            Token::Date => format!("{:04}-{:02}-{:02}", self.year, self.month, self.day),
            Token::Resolution => self.resolution.clone().unwrap_or_else(|| "Other".into()),
            Token::MediaType => self.media_type.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathPattern {
    pub segments: Vec<Vec<Part>>,
}

impl Default for PathPattern {
    fn default() -> Self {
        Self::parse("{Platform}/{Author}/{Year}-{Month}")
    }
}

impl PathPattern {
    /// Parse a template string. Unknown `{tokens}` are kept as literal text.
    pub fn parse(template: &str) -> Self {
        let segments = template
            .split(['/', '\\'])
            .filter(|s| !s.trim().is_empty())
            .map(|seg| {
                let mut parts = Vec::new();
                let mut literal = String::new();
                let mut chars = seg.chars().peekable();
                while let Some(c) = chars.next() {
                    let close = match c {
                        '{' => '}',
                        '[' => ']',
                        _ => {
                            literal.push(c);
                            continue;
                        }
                    };
                    let name: String = chars.by_ref().take_while(|&x| x != close).collect();
                    match Token::parse(&name) {
                        Some(t) => {
                            if !literal.is_empty() {
                                parts.push(Part::Literal(std::mem::take(&mut literal)));
                            }
                            parts.push(Part::Token(t));
                        }
                        None => literal.push_str(&format!("{c}{name}{close}")),
                    }
                }
                if !literal.is_empty() {
                    parts.push(Part::Literal(literal));
                }
                parts
            })
            .collect();
        Self { segments }
    }

    pub fn to_template(&self) -> String {
        self.segments
            .iter()
            .map(|seg| {
                seg.iter()
                    .map(|p| match p {
                        Part::Token(t) => format!("{{{}}}", t.name()),
                        Part::Literal(s) => s.clone(),
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Render to a relative path; every component is sanitized for all OSes.
    pub fn render(&self, ctx: &PatternContext) -> PathBuf {
        self.segments
            .iter()
            .map(|seg| {
                let raw: String = seg
                    .iter()
                    .map(|p| match p {
                        Part::Token(t) => ctx.value(*t),
                        Part::Literal(s) => s.clone(),
                    })
                    .collect();
                sanitize_component(&raw)
            })
            .filter(|c| !c.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> PatternContext {
        PatternContext {
            platform: "YouTube".into(),
            author: Some("MKBHD".into()),
            year: 2027,
            month: 3,
            day: 9,
            resolution: Some("4320p".into()),
            media_type: "Video".into(),
        }
    }

    #[test]
    fn renders_default_pattern() {
        assert_eq!(PathPattern::default().render(&ctx()), PathBuf::from("YouTube/MKBHD/2027-03"));
    }

    #[test]
    fn accepts_bracket_tokens_and_roundtrips() {
        let p = PathPattern::parse("[Platform]/[Resolution]/[Date]");
        assert_eq!(p.render(&ctx()), PathBuf::from("YouTube/4320p/2027-03-09"));
        assert_eq!(p.to_template(), "{Platform}/{Resolution}/{Date}");
    }

    #[test]
    fn hostile_values_cannot_escape_target() {
        let mut c = ctx();
        c.author = Some("../../etc".into());
        let rendered = PathPattern::default().render(&c);
        assert!(rendered.components().all(|x| matches!(x, std::path::Component::Normal(_))), "{rendered:?}");
    }
}
