//! YouTube URL classification.

use url::Url;

use crate::model::ContentKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YouTubeUrl {
    Video {
        id: String,
        kind: ContentKind,
    },
    Playlist {
        list: String,
    },
    /// `/@handle`, `/channel/UC...`, `/c/name`, `/user/name`, optionally with a tab.
    Channel {
        path: String,
    },
}

fn valid_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn classify(url: &Url) -> Option<YouTubeUrl> {
    let host = url.host_str()?.to_ascii_lowercase();
    let segs: Vec<&str> = url.path_segments().map(|s| s.filter(|x| !x.is_empty()).collect()).unwrap_or_default();
    let query = |k: &str| url.query_pairs().find(|(key, _)| key == k).map(|(_, v)| v.into_owned());

    if host.ends_with("youtu.be") {
        let id = segs.first()?;
        return valid_id(id).then(|| YouTubeUrl::Video { id: (*id).to_owned(), kind: ContentKind::Video });
    }

    match segs.as_slice() {
        ["watch", ..] => {
            // A watch URL inside a playlist downloads the single video by default.
            let id = query("v")?;
            valid_id(&id).then_some(YouTubeUrl::Video { id, kind: ContentKind::Video })
        }
        ["shorts", id, ..] if valid_id(id) => {
            Some(YouTubeUrl::Video { id: (*id).to_owned(), kind: ContentKind::Short })
        }
        ["live", id, ..] if valid_id(id) => {
            Some(YouTubeUrl::Video { id: (*id).to_owned(), kind: ContentKind::LiveStream })
        }
        ["embed", id, ..] if valid_id(id) => Some(YouTubeUrl::Video { id: (*id).to_owned(), kind: ContentKind::Video }),
        ["playlist", ..] => query("list").map(|list| YouTubeUrl::Playlist { list }),
        [first, ..] if first.starts_with('@') || matches!(*first, "channel" | "c" | "user") => {
            Some(YouTubeUrl::Channel { path: format!("/{}", segs.join("/")) })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Option<YouTubeUrl> {
        classify(&Url::parse(s).expect("url"))
    }

    #[test]
    fn classifies_urls() {
        assert_eq!(
            c("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=10"),
            Some(YouTubeUrl::Video { id: "dQw4w9WgXcQ".into(), kind: ContentKind::Video })
        );
        assert_eq!(
            c("https://youtube.com/shorts/dQw4w9WgXcQ"),
            Some(YouTubeUrl::Video { id: "dQw4w9WgXcQ".into(), kind: ContentKind::Short })
        );
        assert_eq!(c("https://youtube.com/playlist?list=PL123"), Some(YouTubeUrl::Playlist { list: "PL123".into() }));
        assert_eq!(
            c("https://www.youtube.com/@mkbhd/videos"),
            Some(YouTubeUrl::Channel { path: "/@mkbhd/videos".into() })
        );
        assert_eq!(c("https://www.youtube.com/watch?v=short"), None);
    }
}
