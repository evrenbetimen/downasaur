# yt-dlp EJS challenge solver (vendored)

`lib.min.js` and `core.min.js` come from the `yt-dlp-ejs` 0.8.0 wheel
(<https://github.com/yt-dlp/ejs>), unmodified. The solver parses a YouTube player
script and evaluates its signature (`s`) and throttling (`n`) transforms.

- `core.min.js`: Unlicense (see `LICENSE`).
- `lib.min.js`: bundles meriyah 6.1.4 (ISC) and astring 1.9.0 (MIT); their license
  texts are kept in the file header.

Downasaur runs these scripts in an embedded QuickJS runtime
(`crates/downasaur-core/src/extractors/youtube/jsc.rs`). To update, replace both
files with the ones from a newer `yt-dlp-ejs` release and run the live test
against a current player script:
`DOWNASAUR_PLAYER_JS=/path/to/base.js cargo test -p downasaur-core --release -- --ignored live_player`.
