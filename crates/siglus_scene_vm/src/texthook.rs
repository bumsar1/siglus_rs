//! Opt-in text hook ("Textractor for Mac"): streams the text the engine shows
//! over a local WebSocket, so texthooker pages, GameSentenceMiner and overlays
//! can read it without OCR or DLL hooking.
//!
//! Enabled only when `VNLIB_TEXTHOOK=1`. Serves `ws://127.0.0.1:6677`
//! (`VNLIB_TEXTHOOK_PORT` overrides the port):
//! - any path except `/json`: one plain-text message per completed line
//!   (the Textractor WebSocket extension format);
//! - `/json`: one JSON object per event, `{"kind":"name"|"line"|"choice"|
//!   "clear"|"view", ...}` with window and glyph rectangles in game-screen
//!   pixels and `view`, the game image's rectangle in screen points.
//!
//! When the variable is unset, `enabled()` is a cached `false` and nothing
//! else in this module runs.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("VNLIB_TEXTHOOK")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false);
        if on {
            server::start();
        }
        on
    })
}

/// Called once per UI frame with what the message window currently shows.
/// `glyphs` are the body characters in display order (ruby excluded) with
/// their rectangles; `complete` is true while the window waits for a key.
/// A line is sent once, when it is complete; text added to the same window
/// block afterwards (e.g. after a `PP` wait) is sent as its own line.
pub fn message_frame(
    key: (u32, i64, usize),
    screen: (u32, u32),
    window_rect: Rect,
    name: Option<&str>,
    name_rect: Option<Rect>,
    glyphs: &[(char, Rect)],
    complete: bool,
) {
    #[derive(Default)]
    struct State {
        sent: Vec<char>,
        last_name: Option<String>,
    }
    // Per message window (stage form, stage, MWND index).
    static STATE: OnceLock<Mutex<HashMap<(u32, i64, usize), State>>> = OnceLock::new();
    // Last line sent by any window: a front/back stage copy of the same
    // window during a wipe must not repeat it.
    static LAST_LINE: Mutex<Option<(String, (u32, i64, usize), Instant)>> = Mutex::new(None);
    let mut states = STATE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let st = states.entry(key).or_default();

    let cur: Vec<char> = glyphs.iter().map(|(c, _)| *c).collect();
    if !cur.starts_with(&st.sent) {
        // Window cleared or replaced: the next completed text is a new line.
        if !st.sent.is_empty() {
            // Tells overlays that the text they placed is gone.
            let mut j = json_head("clear", "", screen);
            j.push('}');
            emit("clear", None, j);
        }
        st.sent.clear();
        st.last_name = None;
    }
    if !complete || cur.len() <= st.sent.len() {
        return;
    }
    let start = st.sent.len();
    let new_glyphs = &glyphs[start..];
    let text: String = new_glyphs.iter().map(|(c, _)| *c).collect();
    st.sent = cur;
    let text = text.trim_matches(|c: char| c.is_whitespace());
    if text.is_empty() {
        return;
    }
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    {
        let mut last = LAST_LINE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, k, at)) = last.as_ref()
            && t == text
            && *k != key
            && at.elapsed() < Duration::from_secs(2)
        {
            return;
        }
        *last = Some((text.to_string(), key, Instant::now()));
    }

    if start == 0 && name.is_some() && st.last_name.as_deref() != name {
        let n = name.unwrap_or_default();
        let mut j = json_head("name", n, screen);
        if let Some(r) = name_rect {
            j.push_str(",\"window_rect\":");
            push_rect(&mut j, r);
        }
        j.push('}');
        emit("name", None, j);
    }
    st.last_name = name.map(str::to_string);

    let mut j = json_head("line", text, screen);
    j.push_str(",\"name\":");
    match name {
        Some(n) => push_str(&mut j, n),
        None => j.push_str("null"),
    }
    j.push_str(",\"window_rect\":");
    push_rect(&mut j, window_rect);
    j.push_str(",\"glyph_rects\":[");
    let mut first = true;
    for (c, r) in new_glyphs {
        if c.is_whitespace() && first {
            continue;
        }
        if !first {
            j.push(',');
        }
        first = false;
        j.push_str("{\"ch\":");
        push_str(&mut j, &c.to_string());
        j.push_str(",\"x\":");
        j.push_str(&format!("{},\"y\":{},\"w\":{},\"h\":{}}}", r.x, r.y, r.w, r.h));
    }
    j.push_str("]}");
    emit("line", Some(text.to_string()), j);
}

/// A selection (choice) menu was opened. `rects` may be empty.
pub fn choices(screen: (u32, u32), items: &[String], rects: &[Rect]) {
    let items: Vec<&str> = items
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if items.is_empty() {
        return;
    }
    let plain = items.join("\n");
    let mut j = json_head("choice", &plain, screen);
    j.push_str(",\"choices\":[");
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            j.push(',');
        }
        j.push_str("{\"text\":");
        push_str(&mut j, s);
        j.push_str(",\"rect\":");
        match rects.get(i) {
            Some(r) => push_rect(&mut j, *r),
            None => j.push_str("null"),
        }
        j.push('}');
    }
    j.push_str("]}");
    emit("choice", Some(plain), j);
}

/// JSON events for the text on screen now, replayed to a JSON client that
/// connects later (e.g. an overlay started mid-line).
static SHOWN: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn emit(kind: &str, plain: Option<String>, json: String) {
    {
        let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
        match kind {
            "clear" => shown.clear(),
            "view" => {}
            _ => {
                if shown.len() >= 16 {
                    shown.remove(0);
                }
                shown.push(json.clone());
            }
        }
    }
    server::send(plain, Some(json));
}

fn shown_now() -> Vec<String> {
    SHOWN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

static VIEW: Mutex<Option<[f64; 4]>> = Mutex::new(None);

/// Where the game image is on screen: global top-left points (macOS / CG
/// coordinates), letterbox excluded. A change is sent as a `view` event and
/// every later event carries it, so overlays can map game pixels to the screen.
pub fn set_view(screen: (u32, u32), x: f64, y: f64, w: f64, h: f64) {
    let v = [x, y, w, h].map(|n| (n * 100.0).round() / 100.0);
    {
        let mut cur = VIEW.lock().unwrap_or_else(|e| e.into_inner());
        if *cur == Some(v) {
            return;
        }
        *cur = Some(v);
    }
    let mut j = json_head("view", "", screen);
    j.push('}');
    emit("view", None, j);
}

fn json_head(kind: &str, text: &str, screen: (u32, u32)) -> String {
    let mut j = String::with_capacity(256);
    j.push_str("{\"engine\":\"siglus\",\"kind\":\"");
    j.push_str(kind);
    j.push_str("\",\"text\":");
    push_str(&mut j, text);
    j.push_str(&format!(
        ",\"screen\":{{\"w\":{},\"h\":{}}},\"pid\":{},\"time_ms\":{}",
        screen.0,
        screen.1,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    if let Some([x, y, w, h]) = *VIEW.lock().unwrap_or_else(|e| e.into_inner()) {
        j.push_str(&format!(
            ",\"view\":{{\"x\":{x},\"y\":{y},\"w\":{w},\"h\":{h}}}"
        ));
    }
    j
}

fn push_rect(j: &mut String, r: Rect) {
    j.push_str(&format!(
        "{{\"x\":{},\"y\":{},\"w\":{},\"h\":{}}}",
        r.x, r.y, r.w, r.h
    ));
}

fn push_str(j: &mut String, s: &str) {
    j.push('"');
    for c in s.chars() {
        match c {
            '"' => j.push_str("\\\""),
            '\\' => j.push_str("\\\\"),
            '\n' => j.push_str("\\n"),
            '\r' => j.push_str("\\r"),
            '\t' => j.push_str("\\t"),
            c if (c as u32) < 0x20 => j.push_str(&format!("\\u{:04x}", c as u32)),
            c => j.push(c),
        }
    }
    j.push('"');
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod server {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::{Sender, channel};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    struct Client {
        stream: TcpStream,
        json: bool,
    }

    static TX: OnceLock<Mutex<Sender<(Option<String>, Option<String>)>>> = OnceLock::new();

    pub fn start() {
        let port: u16 = std::env::var("VNLIB_TEXTHOOK_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6677);
        let listener = match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[texthook] cannot listen on 127.0.0.1:{port}: {e}");
                return;
            }
        };
        eprintln!("[texthook] ws://127.0.0.1:{port} (plain) and /json");
        let clients: Arc<Mutex<Vec<Client>>> = Arc::default();
        let (tx, rx) = channel::<(Option<String>, Option<String>)>();
        let _ = TX.set(Mutex::new(tx));

        // "localhost" resolves to ::1 first, and texthooker pages default to
        // ws://localhost:6677, so listen there too when IPv6 is available.
        let mut listeners = vec![listener];
        if let Ok(v6) = TcpListener::bind(("::1", port)) {
            listeners.push(v6);
        }
        for listener in listeners {
            let accept_clients = clients.clone();
            let _ = std::thread::Builder::new()
                .name("texthook-accept".into())
                .spawn(move || {
                    for stream in listener.incoming().flatten() {
                        let clients = accept_clients.clone();
                        let _ = std::thread::Builder::new()
                            .name("texthook-handshake".into())
                            .spawn(move || {
                                if let Some(c) = handshake(stream) {
                                    clients.lock().unwrap_or_else(|e| e.into_inner()).push(c);
                                }
                            });
                    }
                });
        }

        let _ = std::thread::Builder::new()
            .name("texthook-send".into())
            .spawn(move || {
                for (plain, json) in rx {
                    let mut list = clients.lock().unwrap_or_else(|e| e.into_inner());
                    list.retain_mut(|c| {
                        let msg = if c.json { json.as_deref() } else { plain.as_deref() };
                        match msg {
                            Some(m) => c.stream.write_all(&text_frame(m)).is_ok(),
                            None => true,
                        }
                    });
                }
            });
    }

    pub fn send(plain: Option<String>, json: Option<String>) {
        if let Some(tx) = TX.get() {
            let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send((plain, json));
        }
    }

    fn handshake(mut stream: TcpStream) -> Option<Client> {
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut chunk).ok()?;
            if n == 0 || buf.len() > 16 * 1024 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let req = String::from_utf8_lossy(&buf);
        let path = req.lines().next()?.split_whitespace().nth(1)?.to_string();
        let key = req.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("sec-websocket-key")
                .then(|| v.trim().to_string())
        })?;
        let accept = base64(&sha1(
            format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes(),
        ));
        let resp = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
        );
        stream.write_all(resp.as_bytes()).ok()?;
        stream.set_read_timeout(None).ok()?;
        // A stalled client must not hold up the others.
        stream.set_write_timeout(Some(Duration::from_secs(2))).ok()?;
        let json = path.trim_end_matches('/').ends_with("/json") || path.contains("json=1");
        // Drain (and ignore) whatever the client sends, so its socket closes cleanly.
        if let Ok(mut reader) = stream.try_clone() {
            let _ = std::thread::Builder::new()
                .name("texthook-read".into())
                .spawn(move || {
                    let mut b = [0u8; 512];
                    while matches!(reader.read(&mut b), Ok(n) if n > 0) {}
                });
        }
        if json {
            for m in super::shown_now() {
                stream.write_all(&text_frame(&m)).ok()?;
            }
        }
        Some(Client { stream, json })
    }

    fn text_frame(s: &str) -> Vec<u8> {
        let p = s.as_bytes();
        let mut f = Vec::with_capacity(p.len() + 10);
        f.push(0x81);
        if p.len() < 126 {
            f.push(p.len() as u8);
        } else if p.len() <= 0xFFFF {
            f.push(126);
            f.extend_from_slice(&(p.len() as u16).to_be_bytes());
        } else {
            f.push(127);
            f.extend_from_slice(&(p.len() as u64).to_be_bytes());
        }
        f.extend_from_slice(p);
        f
    }

    fn sha1(data: &[u8]) -> [u8; 20] {
        let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
        let mut msg = data.to_vec();
        let bit_len = (data.len() as u64).wrapping_mul(8);
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bit_len.to_be_bytes());
        for block in msg.chunks(64) {
            let mut w = [0u32; 80];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([
                    block[i * 4],
                    block[i * 4 + 1],
                    block[i * 4 + 2],
                    block[i * 4 + 3],
                ]);
            }
            for i in 16..80 {
                w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
            }
            let [mut a, mut b, mut c, mut d, mut e] = h;
            for (i, wi) in w.iter().enumerate() {
                let (f, k) = match i {
                    0..=19 => ((b & c) | (!b & d), 0x5A827999),
                    20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                    40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                    _ => (b ^ c ^ d, 0xCA62C1D6u32),
                };
                let t = a
                    .rotate_left(5)
                    .wrapping_add(f)
                    .wrapping_add(e)
                    .wrapping_add(k)
                    .wrapping_add(*wi);
                e = d;
                d = c;
                c = b.rotate_left(30);
                b = a;
                a = t;
            }
            for (hv, v) in h.iter_mut().zip([a, b, c, d, e]) {
                *hv = hv.wrapping_add(v);
            }
        }
        let mut out = [0u8; 20];
        for (i, v) in h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }

    fn base64(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in data.chunks(3) {
            let n = (c[0] as u32) << 16
                | (*c.get(1).unwrap_or(&0) as u32) << 8
                | *c.get(2).unwrap_or(&0) as u32;
            s.push(T[(n >> 18) as usize & 63] as char);
            s.push(T[(n >> 12) as usize & 63] as char);
            s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
            s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
        }
        s
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn accept_key_matches_rfc6455_example() {
            let k = "dGhlIHNhbXBsZSBub25jZQ==258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
            assert_eq!(super::base64(&super::sha1(k.as_bytes())), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod server {
    pub fn start() {}
    pub fn send(_plain: Option<String>, _json: Option<String>) {}
}
