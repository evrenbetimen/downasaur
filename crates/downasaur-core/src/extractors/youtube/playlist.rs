//! Playlist and channel crawling.
//!
//! Pages embed `ytInitialData`; video IDs are collected from renderer objects and
//! the trailing `continuationCommand.token` is surfaced so the engine can request
//! further pages lazily (large channels can hold tens of thousands of videos).

use serde_json::Value;
use url::Url;

use crate::error::Result;
use crate::extractors::ExtractContext;
use crate::extractors::util::{at_str, js_assigned_json, require};
use crate::model::{Extraction, Platform};
use crate::net::fingerprint::DeviceClass;

const NAME: &str = "YouTube";

pub async fn crawl_playlist(ctx: &ExtractContext, list: &str) -> Result<Extraction> {
    let mut url = Url::parse("https://www.youtube.com/playlist")?;
    url.query_pairs_mut().append_pair("list", list);
    crawl_page(ctx, &url).await
}

pub async fn crawl_channel(ctx: &ExtractContext, path: &str) -> Result<Extraction> {
    let base = Url::parse("https://www.youtube.com")?;
    // Default to the uploads tab when the user pasted a bare channel URL.
    let has_tab = ["/videos", "/shorts", "/streams", "/playlists"].iter().any(|t| path.ends_with(t));
    let url = base.join(&if has_tab { path.to_owned() } else { format!("{path}/videos") })?;
    crawl_page(ctx, &url).await
}

async fn crawl_page(ctx: &ExtractContext, url: &Url) -> Result<Extraction> {
    let html = ctx.http.get_text(url, DeviceClass::Desktop).await?;
    let data = require(js_assigned_json(&html, "ytInitialData"), NAME, "ytInitialData")?;
    let page = parse_initial_data(&data);
    Ok(Extraction::Collection {
        platform: Platform::YouTube,
        title: page.title.unwrap_or_else(|| "YouTube collection".into()),
        entries: page
            .video_ids
            .iter()
            .filter_map(|id| Url::parse(&format!("https://www.youtube.com/watch?v={id}")).ok())
            .collect(),
        continuation: page.continuation,
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CollectionPage {
    pub title: Option<String>,
    pub video_ids: Vec<String>,
    pub continuation: Option<String>,
}

/// Walk `ytInitialData` collecting video IDs (playlist, grid, rich-grid and Shorts
/// renderers) in document order, de-duplicated.
pub fn parse_initial_data(data: &Value) -> CollectionPage {
    let mut page = CollectionPage {
        title: at_str(data, "metadata/playlistMetadataRenderer/title")
            .or_else(|| at_str(data, "metadata/channelMetadataRenderer/title"))
            .map(str::to_owned),
        ..Default::default()
    };
    walk(data, &mut page);
    page
}

fn walk(v: &Value, page: &mut CollectionPage) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                let id = match k.as_str() {
                    "playlistVideoRenderer" | "videoRenderer" | "gridVideoRenderer" => at_str(child, "videoId"),
                    "reelItemRenderer" => at_str(child, "videoId"),
                    "shortsLockupViewModel" => at_str(child, "onTap/innertubeCommand/reelWatchEndpoint/videoId"),
                    "continuationCommand" => {
                        if let Some(t) = at_str(child, "token") {
                            page.continuation = Some(t.to_owned());
                        }
                        None
                    }
                    _ => None,
                };
                if let Some(id) = id.filter(|id| !page.video_ids.iter().any(|x| x == id)) {
                    page.video_ids.push(id.to_owned());
                }
                walk(child, page);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| walk(i, page)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_ids_and_continuation() {
        let data = json!({
            "metadata": { "playlistMetadataRenderer": { "title": "My List" } },
            "contents": [
                { "playlistVideoRenderer": { "videoId": "aaaaaaaaaaa" } },
                { "richItemRenderer": { "content": { "videoRenderer": { "videoId": "bbbbbbbbbbb" } } } },
                { "playlistVideoRenderer": { "videoId": "aaaaaaaaaaa" } },
                { "continuationItemRenderer": { "continuationEndpoint": { "continuationCommand": { "token": "NEXT" } } } }
            ]
        });
        let page = parse_initial_data(&data);
        assert_eq!(page.title.as_deref(), Some("My List"));
        assert_eq!(page.video_ids, vec!["aaaaaaaaaaa", "bbbbbbbbbbb"]);
        assert_eq!(page.continuation.as_deref(), Some("NEXT"));
    }
}
