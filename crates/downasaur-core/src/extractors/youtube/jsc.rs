//! JavaScript challenge solver for the player's signature (`s`) and throttling (`n`) transforms.
//!
//! Stream URLs from JS-dependent clients carry an `n` query parameter that must be
//! rewritten with a function hidden in the player script; without it googlevideo
//! throttles the transfer to roughly real-time speed (unusable for 8K). Ciphered
//! formats additionally need their `s` value transformed into a signature.
//!
//! Both functions are heavily obfuscated, so instead of pattern-matching them we
//! run yt-dlp's EJS solver (vendored under `vendor/yt-dlp-ejs`) in an embedded
//! QuickJS runtime. The solver parses the player into an AST, finds the two
//! transforms structurally and evaluates them on our challenges.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use rquickjs::{CatchResultExt, Context, Runtime};
use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::error::{CoreError, Result};
use crate::net::HttpPool;
use crate::net::fingerprint::DeviceClass;

const NAME: &str = "YouTube";
const LIB_JS: &str = include_str!("../../../vendor/yt-dlp-ejs/lib.min.js");
const CORE_JS: &str = include_str!("../../../vendor/yt-dlp-ejs/core.min.js");

/// Browser behaviour QuickJS lacks. Players since `f2999a12` stringify an array
/// that contains itself: V8 renders the cycle as `""`, while QuickJS recurses in
/// native code until the thread's stack is gone (the JS stack limit never fires).
const PRELUDE_JS: &str = r#"(() => {
  const join = Array.prototype.join, active = new Set();
  Object.defineProperty(Array.prototype, "join", {
    writable: true, configurable: true,
    value: function (sep) {
      if (active.has(this)) return "";
      active.add(this);
      try { return join.call(this, sep); } finally { active.delete(this); }
    },
  });
})();"#;

/// QuickJS keeps its parser recursion on the native stack; the player AST is deep.
const THREAD_STACK: usize = 64 * 1024 * 1024;
const JS_MEMORY_LIMIT: usize = 1024 * 1024 * 1024;

/// Preprocessed player scripts keyed by player id; preprocessing (parse + rewrite)
/// is the expensive part, evaluation afterwards is cheap.
static PREPROCESSED: LazyLock<Mutex<HashMap<String, Arc<String>>>> = LazyLock::new(Default::default);

/// Solved challenge values, input → output.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Solutions {
    pub n: HashMap<String, String>,
    pub sig: HashMap<String, String>,
}

/// Solve `n` and `sig` challenges against the player script `player_js`
/// (identified by `player_id` for caching). Runs on a dedicated thread.
pub async fn solve(player_id: &str, player_js: Arc<String>, n: Vec<String>, sig: Vec<String>) -> Result<Solutions> {
    if n.is_empty() && sig.is_empty() {
        return Ok(Solutions::default());
    }
    let cached = PREPROCESSED.lock().get(player_id).cloned();
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new().name("yt-jsc".into()).stack_size(THREAD_STACK).spawn(move || {
        let _ = tx.send(solve_blocking(&player_js, cached.as_deref().map(String::as_str), &n, &sig));
    })?;
    let (solutions, preprocessed) =
        rx.await.map_err(|_| CoreError::extraction(NAME, "challenge solver thread died"))??;
    if let Some(p) = preprocessed {
        PREPROCESSED.lock().insert(player_id.to_owned(), Arc::new(p));
    }
    Ok(solutions)
}

#[derive(Debug, Deserialize)]
struct SolverOutput {
    #[serde(rename = "type")]
    kind: String,
    error: Option<String>,
    #[serde(default)]
    responses: Vec<SolverResponse>,
    preprocessed_player: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SolverResponse {
    #[serde(rename = "type")]
    kind: String,
    error: Option<String>,
    #[serde(default)]
    data: HashMap<String, String>,
}

/// Synchronous core: returns the solutions plus the preprocessed player when it
/// was produced by this call (so the caller can cache it).
pub fn solve_blocking(
    player_js: &str,
    preprocessed: Option<&str>,
    n: &[String],
    sig: &[String],
) -> Result<(Solutions, Option<String>)> {
    let js_err = |what: &str, detail: String| CoreError::extraction(NAME, format!("challenge solver {what}: {detail}"));

    let rt = Runtime::new().map_err(|e| js_err("runtime", e.to_string()))?;
    rt.set_memory_limit(JS_MEMORY_LIMIT);
    rt.set_max_stack_size(THREAD_STACK - 4 * 1024 * 1024);
    let ctx = Context::full(&rt).map_err(|e| js_err("context", e.to_string()))?;

    let requests = json!([
        { "type": "n", "challenges": n },
        { "type": "sig", "challenges": sig },
    ])
    .to_string();
    let (input_kind, player) = match preprocessed {
        Some(p) => ("preprocessed", p),
        None => ("player", player_js),
    };

    let raw: String = ctx.with(|ctx| -> Result<String> {
        let globals = ctx.globals();
        globals.set("__player", player).map_err(|e| js_err("input", e.to_string()))?;
        globals.set("__requests", requests.as_str()).map_err(|e| js_err("input", e.to_string()))?;
        let script = format!(
            "{PRELUDE_JS}\n{LIB_JS}\n;Object.assign(globalThis, lib);\n{CORE_JS}\n;\
             JSON.stringify(jsc({{type: {input_kind:?}, player: __player, preprocessed_player: __player, \
             output_preprocessed: {output}, requests: JSON.parse(__requests)}}));",
            output = preprocessed.is_none(),
        );
        ctx.eval::<String, _>(script).catch(&ctx).map_err(|e| js_err("failed", e.to_string()))
    })?;

    let out: SolverOutput = serde_json::from_str(&raw).map_err(|e| js_err("output", e.to_string()))?;
    if out.kind != "result" {
        return Err(js_err("failed", out.error.unwrap_or(out.kind)));
    }
    let mut solutions = Solutions::default();
    // Responses come back in request order: n, then sig.
    for (resp, slot) in out.responses.into_iter().zip([&mut solutions.n, &mut solutions.sig]) {
        if resp.kind != "result" {
            return Err(js_err("failed", resp.error.unwrap_or(resp.kind)));
        }
        *slot = resp.data;
    }
    Ok((solutions, out.preprocessed_player))
}

/// Raw player scripts keyed by player id (a few MB each; ids rotate slowly).
static PLAYERS: LazyLock<Mutex<HashMap<String, Arc<String>>>> = LazyLock::new(Default::default);

/// The current web player script.
#[derive(Debug, Clone)]
pub struct PlayerScript {
    pub id: String,
    pub js: Arc<String>,
    pub signature_timestamp: Option<u64>,
}

impl PlayerScript {
    /// Find the current player id via the lightweight iframe API loader and fetch
    /// its `base.js` (cached per id for the lifetime of the process).
    pub async fn load(http: &HttpPool) -> Result<Self> {
        let api = Url::parse("https://www.youtube.com/iframe_api")?;
        let loader = http.get_text(&api, DeviceClass::Desktop).await?.replace("\\/", "/");
        let id = player_id_from_path(&loader)
            .ok_or_else(|| CoreError::extraction(NAME, "player id not found in iframe API"))?
            .to_owned();

        let cached = PLAYERS.lock().get(&id).cloned();
        let js = match cached {
            Some(js) => js,
            None => {
                let url =
                    Url::parse(&format!("https://www.youtube.com/s/player/{id}/player_ias.vflset/en_US/base.js"))?;
                let js = Arc::new(http.get_text(&url, DeviceClass::Desktop).await?);
                PLAYERS.lock().insert(id.clone(), js.clone());
                js
            }
        };
        tracing::debug!(player = %id, bytes = js.len(), "youtube player script loaded");
        Ok(Self { signature_timestamp: signature_timestamp(&js), id, js })
    }

    pub async fn solve(&self, n: Vec<String>, sig: Vec<String>) -> Result<Solutions> {
        solve(&self.id, self.js.clone(), n, sig).await
    }
}

/// Player id from a script path like `/s/player/5203c085/player_ias.vflset/en_US/base.js`.
pub fn player_id_from_path(path: &str) -> Option<&str> {
    let rest = &path[path.find("/player/")? + "/player/".len()..];
    let id = &rest[..rest.find('/')?];
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(id)
}

/// `signatureTimestamp` the player was built for; sent with InnerTube requests so
/// the returned ciphers match this player.
pub fn signature_timestamp(player_js: &str) -> Option<u64> {
    let i = player_js.find("signatureTimestamp:").map(|i| i + "signatureTimestamp:".len())?;
    let digits: String = player_js[i..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_player_id() {
        assert_eq!(player_id_from_path("/s/player/5203c085/player_ias.vflset/en_US/base.js"), Some("5203c085"));
        assert_eq!(player_id_from_path("/s/player//base.js"), None);
        assert_eq!(player_id_from_path("/yts/jsbin/base.js"), None);
    }

    #[test]
    fn finds_signature_timestamp() {
        assert_eq!(signature_timestamp("a={signatureTimestamp:20367,foo:1}"), Some(20367));
        assert_eq!(signature_timestamp("nothing here"), None);
    }

    #[test]
    fn empty_requests_skip_the_runtime() {
        let rt = tokio::runtime::Builder::new_current_thread().build().expect("rt");
        let out = rt.block_on(solve("x", Arc::new(String::new()), vec![], vec![])).expect("ok");
        assert_eq!(out, Solutions::default());
    }

    /// The solver must load and report a structured error on a script that has no
    /// player functions, instead of crashing the runtime.
    #[test]
    fn reports_missing_functions() {
        let src = "var _yt_player={};(function(g){var window=this;var a=1;})(_yt_player);";
        let err = solve_blocking(src, None, &["abc".into()], &[]).expect_err("no n function");
        assert!(err.to_string().contains("challenge solver"), "{err}");
    }

    /// A self-referencing array must stringify like V8 (`""` for the cycle)
    /// instead of overflowing the native stack.
    #[test]
    fn cyclic_array_join_terminates() {
        let src = "var _yt_player={};(function(g){var window=this;})(_yt_player);";
        let rt = Runtime::new().expect("rt");
        let ctx = Context::full(&rt).expect("ctx");
        let out: String = ctx.with(|ctx| {
            ctx.eval::<(), _>(PRELUDE_JS).expect("prelude");
            ctx.eval("var a = [1, 2]; a.push(a); String(a) + '|' + [a, [3]].join('-')").expect("eval")
        });
        assert_eq!(out, "1,2,|1,2,-3");
        // The solver still reports missing functions cleanly with the prelude in place.
        assert!(solve_blocking(src, None, &["abc".into()], &[]).is_err());
    }

    /// Solves real challenges against a downloaded player script.
    /// `DOWNASAUR_PLAYER_JS=/path/to/base.js cargo test -p downasaur-core -- --ignored live_player`
    #[test]
    #[ignore = "needs a real player script in DOWNASAUR_PLAYER_JS"]
    fn live_player() {
        let path = std::env::var("DOWNASAUR_PLAYER_JS").expect("DOWNASAUR_PLAYER_JS");
        let js = std::fs::read_to_string(path).expect("player");
        let t = std::time::Instant::now();
        let (out, pre) = solve_blocking(&js, None, &["ZdZIqFPQK-Ty8wId".into()], &["gN7a-hudCuAuPH6fByOk1_GNXN0yNMHShjZXS2VOgsEItAJz0tipeavEOmNdYN-wUtcEqD3bCXjc0iyKfAyZxCBGgIARwsSdQfJ2CJtt".into()]).expect("solved");
        eprintln!("solved in {:?}: {out:?}", t.elapsed());
        assert_eq!(out.n.len(), 1);
        assert_ne!(out.n["ZdZIqFPQK-Ty8wId"], "ZdZIqFPQK-Ty8wId");
        assert_eq!(out.sig.len(), 1);
        let (again, _) = solve_blocking(&js, pre.as_deref(), &["ZdZIqFPQK-Ty8wId".into()], &[]).expect("cached");
        assert_eq!(again.n, out.n);
    }
}
