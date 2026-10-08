//! Parsing helpers shared by the HTML/JSON scraping extractors.

use regex::Regex;
use serde_json::Value;

use crate::error::{CoreError, Result};

/// Extract and parse the JSON body of `<script id="{id}" ...>{json}</script>`.
pub fn script_json_by_id(html: &str, id: &str) -> Option<Value> {
    let marker = format!("id=\"{id}\"");
    let start = html.find(&marker)?;
    let open_end = start + html[start..].find('>')? + 1;
    let close = open_end + html[open_end..].find("</script>")?;
    serde_json::from_str(&html[open_end..close]).ok()
}

/// All `<script type="application/ld+json">` blocks, parsed.
pub fn json_ld_blocks(html: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("application/ld+json") {
        rest = &rest[pos..];
        let Some(open) = rest.find('>') else { break };
        let body = &rest[open + 1..];
        let Some(close) = body.find("</script>") else { break };
        if let Ok(v) = serde_json::from_str(&body[..close]) {
            out.push(v);
        }
        rest = &body[close..];
    }
    out
}

/// Find a JS assignment like `var ytInitialData = {...};` and parse its object literal.
pub fn js_assigned_json(html: &str, var: &str) -> Option<Value> {
    let start = html.find(var)?;
    let brace = start + html[start..].find('{')?;
    let end = balanced_object_end(&html[brace..])?;
    serde_json::from_str(&html[brace..brace + end]).ok()
}

/// Length of the balanced `{...}` object at the start of `s`, string-aware.
fn balanced_object_end(s: &str) -> Option<usize> {
    let (mut depth, mut in_str, mut escaped) = (0usize, false, false);
    for (i, b) in s.bytes().enumerate() {
        if in_str {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find `"key":"<json string>"` in raw HTML and return the unescaped string.
pub fn json_string_field(html: &str, key: &str) -> Option<String> {
    let re = Regex::new(&format!(r#""{}"\s*:\s*("(?:[^"\\]|\\.)*")"#, regex::escape(key))).ok()?;
    let raw = re.captures(html)?.get(1)?.as_str();
    serde_json::from_str::<String>(raw).ok().filter(|s| !s.is_empty())
}

/// Navigate a JSON value by a `/`-separated path (`a/0/b`).
pub fn at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('/').filter(|p| !p.is_empty()).try_fold(v, |cur, seg| match seg.parse::<usize>() {
        Ok(i) => cur.get(i),
        Err(_) => cur.get(seg),
    })
}

pub fn at_str<'a>(v: &'a Value, path: &str) -> Option<&'a str> {
    at(v, path).and_then(Value::as_str)
}

pub fn at_u64(v: &Value, path: &str) -> Option<u64> {
    at(v, path).and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
}

pub fn parse_err(platform: &'static str, what: &str) -> CoreError {
    CoreError::extraction(platform, format!("could not locate {what} in the page"))
}

pub fn require<T>(v: Option<T>, platform: &'static str, what: &str) -> Result<T> {
    v.ok_or_else(|| parse_err(platform, what))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_assigned_object_with_braces_in_strings() {
        let html = r#"<script>var ytInitialData = {"a":"}{","b":{"c":1}};</script>"#;
        let v = js_assigned_json(html, "ytInitialData").expect("parsed");
        assert_eq!(at_u64(&v, "b/c"), Some(1));
    }

    #[test]
    fn extracts_script_by_id() {
        let html = r#"<script id="__DATA__" type="application/json">{"x":[{"y":"z"}]}</script>"#;
        let v = script_json_by_id(html, "__DATA__").expect("parsed");
        assert_eq!(at_str(&v, "x/0/y"), Some("z"));
    }

    #[test]
    fn unescapes_string_field() {
        let html = r#"{"playable_url_quality_hd":"https:\/\/video.example\/v.mp4?a=1&b=2"}"#;
        assert_eq!(
            json_string_field(html, "playable_url_quality_hd").as_deref(),
            Some("https://video.example/v.mp4?a=1&b=2")
        );
    }
}
