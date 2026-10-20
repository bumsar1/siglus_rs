# VNLIB text hook (siglus_engine)

Local patch (not upstream). It works like Textractor for Mac: the engine streams
the text it shows over a local WebSocket. It reads the engine's own state, so
there is no DLL hooking and no OCR.

## Use

    VNLIB_TEXTHOOK=1 target/release/siglus_engine --project-dir "<game>"
    python3 tools/texthook_client.py /       # plain lines
    python3 tools/texthook_client.py /json   # JSON events

- Listens on `127.0.0.1:6677`. `VNLIB_TEXTHOOK_PORT` overrides the port. Port
  6677 is the Textractor WebSocket extension's default, so GameSentenceMiner
  and texthooker pages connect without extra setup.
- When `VNLIB_TEXTHOOK` is unset (or `0`), the check is a cached `false`.
  No thread starts and no socket opens.
- Every path except `/json` gets one plain-text message per completed line,
  with no speaker name. A choice menu comes as one message, one option per line.
- `/json` (or `?json=1`) gets one JSON object per event.

## Contract (shared with the KiriKiri/BGI players, VN Library, Meikipop)

```json
{"engine":"siglus","kind":"name","text":"声","screen":{"w":1920,"h":1080},"time_ms":…,
 "window_rect":{"x":377,"y":776,"w":526,"h":65}}
{"engine":"siglus","kind":"line","text":"「おきてください」","screen":{"w":1920,"h":1080},"time_ms":…,
 "name":"声","window_rect":{"x":7,"y":727,"w":1906,"h":353},
 "glyph_rects":[{"ch":"「","x":369,"y":851,"w":38,"h":38}, …]}
{"engine":"siglus","kind":"choice","text":"鏡子さん\nおばーちゃん\n…","screen":{…},"time_ms":…,
 "choices":[{"text":"鏡子さん","rect":null}, …]}
```

- `kind`: `name` (sent before a line whose window shows a speaker), `line`, or `choice`.
- All rectangles are in **game-screen pixels** (`screen` = the VM resolution,
  for example 1920x1080). The window is letterboxed, so an overlay must map these
  into the window's content area itself. `glyph_rects` has one entry per
  character of `text`, in order. Ruby and emoji glyphs are left out. `w`/`h` is
  the font cell (`size`), so half-width characters come out too wide.
- `name.window_rect` is the name frame. `line.window_rect` is the message window frame.
- `choice.rect` is `null` for now. Neither selection path computes the item
  positions before the menu opens.

## When a line counts as sent

A line is sent once, when its message window shows the key-wait icon
(`MwndState.key_icon_appear`, set by PP/R/PAGE). By then the whole text is
known, even while it is still revealing character by character. The tracking
is per window `(stage form, stage, mwnd)`. Text appended to the same block
after a PP wait is sent as its own line. A cleared window starts over. If a
second window (a back-stage copy during a wipe) shows the same text within
2 s, it is dropped.

## Changed files

- `crates/siglus_scene_vm/src/texthook.rs`: new. Has the env check,
  dedup/state, JSON, and the std-only WebSocket server (accept, send and
  per-client drain threads, plus a built-in SHA-1/base64 for the handshake).
  No new crate dependencies. On non-desktop targets the server is a stub.
- `src/lib.rs`: `pub mod texthook;`
- `src/runtime/ui.rs`: `UiRuntime::texthook_frame(key, w, h)` builds glyph
  rects from the projected MWND glyphs (`msg_rect` origin + glyph x/y + slide).
  `tick()` calls it after `tick_additional_mwnds` for the primary window and
  for every child instance. A child whose key equals `primary_mwnd_key` is
  skipped, because it is a stale copy. Hooking inside `tick_mwnd_only` instead
  re-sent every line each frame.
- `src/runtime/forms/stage.rs`: MWND `SEL*` sends `choice`.
- `src/runtime/forms/global.rs`: `SELBTN` start sends `choice`. Summer Pockets uses this path.

Branch `vnlib-texthook` in the private repo github.com/bumsar1/siglus_rs
(`upstream` = xmoezzz/siglus_rs; VN Library merges upstream/main into it on update).

## Verified (2026-10-20)

Tested with Summer Pockets REFLECTION BLUE, `--scene-name 10_プロローグ0726`,
110 Return presses posted to the process (CGEvent.postToPid, no focus change).
The result was 101 lines and 1 choice (4 options), with no duplicates. The
text and glyph positions match a screenshot. With the variable unset, nothing
listens on 6677.

## Ideas / next

- Window→screen mapping (letterbox offset/scale, macOS window frame in points)
  so Meikipop doesn't need its own; pump_host knows it.
- Real advance widths per glyph; choice rects from SELBTN layout.
- Same contract in VN-Ports KiriKiri/BGI players; VN Library Text panel.
