use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const BG_USER_AGENT_FALLBACK: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const VISITOR_TTL: Duration = Duration::from_secs(6 * 3600);
const PLAYER_TTL: Duration = Duration::from_secs(6 * 3600);

pub trait JsHost: Send + Sync {
    fn eval_async_json(&self, script_body: &str, timeout: Duration) -> Result<Value, String>;
    fn eval_sync_json(&self, expr: &str, timeout: Duration) -> Result<Value, String>;
}

pub struct CallbackBridge {
    port: u16,
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>,
    next_id: AtomicU64,
    server: Arc<tiny_http::Server>,
    stop: Arc<AtomicBool>,
}

impl CallbackBridge {
    pub fn start() -> Result<Self, String> {
        let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| "callback server missing ip".to_string())?
            .port();
        let server = Arc::new(server);
        let pending: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let srv = server.clone();
        let pnd = pending.clone();
        let stp = stop.clone();
        std::thread::spawn(move || {
            for mut request in srv.incoming_requests() {
                if stp.load(Ordering::Acquire) {
                    break;
                }
                let url = request.url().split('?').next().unwrap_or("");
                let Some(req_id) = url.strip_prefix("/_bg/") else {
                    let _ = request.respond(
                        tiny_http::Response::from_string("not found").with_status_code(404),
                    );
                    continue;
                };
                let req_id = req_id.to_string();

                let cors_headers = vec![
                    tiny_http::Header::from_bytes(b"Access-Control-Allow-Origin", b"*").unwrap(),
                    tiny_http::Header::from_bytes(
                        b"Access-Control-Allow-Methods",
                        b"POST, OPTIONS",
                    )
                    .unwrap(),
                    tiny_http::Header::from_bytes(
                        b"Access-Control-Allow-Headers",
                        b"content-type",
                    )
                    .unwrap(),
                ];

                if request.method() == &tiny_http::Method::Options {
                    let resp = tiny_http::Response::new(
                        tiny_http::StatusCode(200),
                        cors_headers,
                        std::io::empty(),
                        Some(0),
                        None,
                    );
                    let _ = request.respond(resp);
                    continue;
                }

                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                if let Some(tx) = pnd.lock().unwrap().remove(&req_id) {
                    let _ = tx.send(body);
                }
                let resp = tiny_http::Response::new(
                    tiny_http::StatusCode(200),
                    cors_headers,
                    std::io::Cursor::new(b"ok".to_vec()),
                    Some(2),
                    None,
                );
                let _ = request.respond(resp);
            }
        });

        Ok(Self {
            port,
            pending,
            next_id: AtomicU64::new(1),
            server,
            stop,
        })
    }

    pub fn register(&self) -> (String, String, mpsc::Receiver<String>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let url = format!("http://127.0.0.1:{}/_bg/{}", self.port, id);
        (id, url, rx)
    }

    pub fn cancel(&self, id: &str) {
        self.pending.lock().unwrap().remove(id);
    }
}

impl Drop for CallbackBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.server.unblock();
    }
}

pub fn wrap_async_script(script_body: &str, cb_url: &str) -> String {
    let cb_json = serde_json::to_string(cb_url).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"(async () => {{
  const __cb = {cb_json};
  try {{
    const __res = await (async () => {{
{script_body}
    }})();
    await fetch(__cb, {{
      method: "POST",
      headers: {{ "content-type": "application/json" }},
      body: JSON.stringify({{ ok: true, value: __res }})
    }});
  }} catch (__err) {{
    try {{
      await fetch(__cb, {{
        method: "POST",
        headers: {{ "content-type": "application/json" }},
        body: JSON.stringify({{ ok: false, error: String(__err && (__err.stack || __err.message || __err)) }})
      }});
    }} catch (_) {{}}
  }}
}})();"#
    )
}

pub fn unwrap_callback_payload(raw: &str) -> Result<Value, String> {
    let parsed: Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid callback json: {e}"))?;
    if parsed.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(parsed.get("value").cloned().unwrap_or(Value::Null))
    } else {
        let err = parsed
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown JS error");
        Err(err.to_string())
    }
}

pub fn parse_sync_json_result(raw: &str) -> Result<Value, String> {
    if raw.is_empty() {
        return Err("empty eval_with_callback result (possible JS exception)".to_string());
    }
    let val: Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid sync eval json: {e}"))?;
    if let Some(inner) = val.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Value>(inner) {
            return Ok(parsed);
        }
    }
    Ok(val)
}

const QUICKJS_POLYFILLS: &str = r#"
(() => {
  const g = globalThis;
  g.window = g;
  g.self = g;
  g.top = g;
  g.parent = g;
  g.location = {
    href: "https://www.youtube.com/robots.txt",
    origin: "https://www.youtube.com",
    protocol: "https:",
    host: "www.youtube.com",
    hostname: "www.youtube.com",
    pathname: "/robots.txt",
    search: "",
    hash: "",
    toString() { return this.href; }
  };
  g.navigator = {
    userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    language: "en-US",
    languages: ["en-US", "en"],
    platform: "Win32",
    vendor: "Google Inc.",
    onLine: true,
    hardwareConcurrency: 8,
    deviceMemory: 8,
    webdriver: false,
    maxTouchPoints: 0,
    cookieEnabled: true
  };
  g.screen = { width: 1920, height: 1080, availWidth: 1920, availHeight: 1040, colorDepth: 24, pixelDepth: 24 };
  g.devicePixelRatio = 1;
  g.innerWidth = 430;
  g.innerHeight = 640;
  g.outerWidth = 430;
  g.outerHeight = 640;
  g.performance = {
    timeOrigin: Date.now(),
    now() { return Date.now() - this.timeOrigin; }
  };
  function makeEl(tag) {
    return {
      tagName: String(tag || "DIV").toUpperCase(),
      nodeType: 1,
      style: {},
      children: [],
      childNodes: [],
      dataset: {},
      appendChild(c) { this.children.push(c); return c; },
      removeChild(c) { return c; },
      insertBefore(c) { this.children.push(c); return c; },
      setAttribute(k, v) { this[k] = v; },
      getAttribute(k) { return this[k] ?? null; },
      removeAttribute(k) { delete this[k]; },
      addEventListener() {},
      removeEventListener() {},
      getElementsByTagName() { return []; },
      querySelector() { return null; },
      querySelectorAll() { return []; },
      cloneNode() { return makeEl(tag); },
      getContext() {
        return {
          fillRect() {}, clearRect() {}, getImageData() { return { data: new Uint8ClampedArray(16) }; },
          putImageData() {}, fillText() {}, measureText() { return { width: 10 }; },
          beginPath() {}, arc() {}, fill() {}, stroke() {}, closePath() {}
        };
      },
      toDataURL() { return "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="; }
    };
  }
  const docEl = makeEl("html");
  const headEl = makeEl("head");
  const bodyEl = makeEl("body");
  g.document = {
    nodeType: 9,
    readyState: "complete",
    URL: g.location.href,
    documentURI: g.location.href,
    domain: "www.youtube.com",
    referrer: "",
    cookie: "",
    title: "",
    hidden: false,
    visibilityState: "visible",
    location: g.location,
    documentElement: docEl,
    head: headEl,
    body: bodyEl,
    createElement(t) { return makeEl(t); },
    createElementNS(_ns, t) { return makeEl(t); },
    createTextNode(t) { return { nodeType: 3, textContent: String(t) }; },
    createDocumentFragment() { return makeEl("fragment"); },
    createEvent() { return { initEvent() {} }; },
    getElementById() { return null; },
    getElementsByTagName(t) {
      const u = String(t || "").toLowerCase();
      if (u === "head") return [headEl];
      if (u === "body") return [bodyEl];
      return [];
    },
    getElementsByClassName() { return []; },
    querySelector(s) {
      if (s === "head") return headEl;
      if (s === "body") return bodyEl;
      return null;
    },
    querySelectorAll() { return []; },
    addEventListener() {},
    removeEventListener() {}
  };
  g.Element = function Element() {};
  g.HTMLElement = function HTMLElement() {};
  g.Node = function Node() {};
  g.Event = function Event(t) { this.type = t; };
  g.CustomEvent = function CustomEvent(t, o) { this.type = t; this.detail = o && o.detail; };
  g.MutationObserver = function MutationObserver() { this.observe = () => {}; this.disconnect = () => {}; };
  g.XMLHttpRequest = function XMLHttpRequest() {};
  g.addEventListener = function () {};
  g.removeEventListener = function () {};
  g.postMessage = function () {};
  g.setTimeout = function (fn) {
    if (typeof fn === "function") Promise.resolve().then(() => { try { fn(); } catch (_) {} });
    return 1;
  };
  g.clearTimeout = function () {};
  g.setInterval = function () { return 1; };
  g.clearInterval = function () {};
  g.requestAnimationFrame = function (fn) {
    if (typeof fn === "function") Promise.resolve().then(() => { try { fn(g.performance.now()); } catch (_) {} });
    return 1;
  };
  g.cancelAnimationFrame = function () {};

  const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  g.btoa = function btoa(str) {
    const s = String(str);
    let out = "";
    for (let i = 0; i < s.length; i += 3) {
      const c1 = s.charCodeAt(i) & 0xff;
      const h2 = i + 1 < s.length;
      const h3 = i + 2 < s.length;
      const c2 = h2 ? (s.charCodeAt(i + 1) & 0xff) : 0;
      const c3 = h3 ? (s.charCodeAt(i + 2) & 0xff) : 0;
      const n = (c1 << 16) | (c2 << 8) | c3;
      out += B64[(n >> 18) & 63] + B64[(n >> 12) & 63] + (h2 ? B64[(n >> 6) & 63] : "=") + (h3 ? B64[n & 63] : "=");
    }
    return out;
  };
  g.atob = function atob(input) {
    let s = String(input).replace(/[\t\n\f\r ]+/g, "");
    if (s.length % 4 === 1) throw new Error("Invalid base64");
    let out = "";
    let bc = 0, bs = 0;
    for (let i = 0; i < s.length; i++) {
      const ch = s.charAt(i);
      if (ch === "=") break;
      const idx = B64.indexOf(ch);
      if (idx < 0) continue;
      bs = bc % 4 ? bs * 64 + idx : idx;
      if (bc++ % 4) out += String.fromCharCode(255 & (bs >> ((-2 * bc) & 6)));
    }
    return out;
  };

  g.TextEncoder = class TextEncoder {
    encode(str = "") {
      const s = String(str);
      const bytes = [];
      for (let i = 0; i < s.length; i++) {
        let code = s.charCodeAt(i);
        if (code < 0x80) {
          bytes.push(code);
        } else if (code < 0x800) {
          bytes.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
        } else if (code >= 0xd800 && code <= 0xdbff && i + 1 < s.length) {
          const low = s.charCodeAt(++i);
          const cp = ((code - 0xd800) << 10) + (low - 0xdc00) + 0x10000;
          bytes.push(0xf0 | (cp >> 18), 0x80 | ((cp >> 12) & 0x3f), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f));
        } else {
          bytes.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
        }
      }
      return new Uint8Array(bytes);
    }
  };

  g.TextDecoder = class TextDecoder {
    decode(input) {
      if (!input) return "";
      const bytes = input instanceof Uint8Array ? input : new Uint8Array(input.buffer || input);
      let out = "";
      let i = 0;
      while (i < bytes.length) {
        const b1 = bytes[i++];
        if (b1 < 0x80) {
          out += String.fromCharCode(b1);
        } else if ((b1 & 0xe0) === 0xc0) {
          const b2 = bytes[i++] & 0x3f;
          out += String.fromCharCode(((b1 & 0x1f) << 6) | b2);
        } else if ((b1 & 0xf0) === 0xe0) {
          const b2 = bytes[i++] & 0x3f;
          const b3 = bytes[i++] & 0x3f;
          out += String.fromCharCode(((b1 & 0x0f) << 12) | (b2 << 6) | b3);
        } else {
          const b2 = bytes[i++] & 0x3f;
          const b3 = bytes[i++] & 0x3f;
          const b4 = bytes[i++] & 0x3f;
          const cp = (((b1 & 0x07) << 18) | (b2 << 12) | (b3 << 6) | b4) - 0x10000;
          out += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
        }
      }
      return out;
    }
  };

  if (!g.crypto) {
    g.crypto = {
      getRandomValues(arr) {
        for (let i = 0; i < arr.length; i++) arr[i] = Math.floor(Math.random() * 256);
        return arr;
      }
    };
  }

  g.fetch = async function fetch(url, init) {
    const reqObj = {
      url: String(url),
      method: (init && init.method) ? String(init.method) : "GET",
      headers: (init && init.headers) ? init.headers : {},
      body: (init && init.body != null) ? String(init.body) : null
    };
    const rawResp = g.__nada_native_fetch(JSON.stringify(reqObj));
    const parsed = JSON.parse(rawResp);
    if (!parsed.ok_transport) {
      throw new Error(parsed.error || "fetch transport error");
    }
    const status = parsed.status || 200;
    const textBody = parsed.body || "";
    return {
      ok: status >= 200 && status < 300,
      status,
      url: String(url),
      async text() { return textBody; },
      async json() { return JSON.parse(textBody); }
    };
  };
})();
"#;

enum QjsCmd {
    EvalAsync(String, Duration, mpsc::Sender<Result<Value, String>>),
    EvalSync(String, Duration, mpsc::Sender<Result<Value, String>>),
}

pub struct QuickJsHost {
    tx: mpsc::Sender<QjsCmd>,
}

fn execute_native_fetch(client: &reqwest::blocking::Client, req_json: String) -> String {
    let parsed: Value = match serde_json::from_str(&req_json) {
        Ok(v) => v,
        Err(e) => {
            return serde_json::json!({
                "ok_transport": false,
                "error": format!("invalid fetch req json: {e}")
            })
            .to_string();
        }
    };
    let url = parsed.get("url").and_then(Value::as_str).unwrap_or("");
    let method = parsed
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET");
    let mut builder = match method.to_ascii_uppercase().as_str() {
        "POST" => client.post(url),
        "HEAD" => client.head(url),
        "OPTIONS" => client.request(reqwest::Method::OPTIONS, url),
        _ => client.get(url),
    };
    builder = builder
        .header(reqwest::header::USER_AGENT, BG_USER_AGENT_FALLBACK)
        .header(reqwest::header::ORIGIN, "https://www.youtube.com")
        .header(reqwest::header::REFERER, "https://www.youtube.com/");

    if let Some(hdrs) = parsed.get("headers").and_then(Value::as_object) {
        for (k, v) in hdrs {
            if let Some(vs) = v.as_str() {
                builder = builder.header(k.as_str(), vs);
            }
        }
    }
    if let Some(body) = parsed.get("body").and_then(Value::as_str) {
        builder = builder.body(body.to_string());
    }

    match builder.send() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().unwrap_or_default();
            serde_json::json!({
                "ok_transport": true,
                "status": status,
                "body": body
            })
            .to_string()
        }
        Err(e) => serde_json::json!({
            "ok_transport": false,
            "error": e.to_string()
        })
        .to_string(),
    }
}

impl QuickJsHost {
    pub fn start() -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<QjsCmd>();
        let (init_tx, init_rx) = mpsc::channel::<Result<(), String>>();

        std::thread::Builder::new()
            .name("nada-qjs-host".to_string())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let bridge = match CallbackBridge::start() {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                        return;
                    }
                };
                let http = match reqwest::blocking::Client::builder()
                    .connect_timeout(Duration::from_secs(10))
                    .timeout(Duration::from_secs(20))
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = init_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let rt = match rquickjs::Runtime::new() {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = init_tx.send(Err(format!("QuickJS runtime init failed: {e}")));
                        return;
                    }
                };
                rt.set_max_stack_size(4 * 1024 * 1024);
                let ctx = match rquickjs::Context::full(&rt) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = init_tx.send(Err(format!("QuickJS context init failed: {e}")));
                        return;
                    }
                };

                let poly_res: Result<(), String> = ctx.with(|ctx| {
                    let http_cl = http.clone();
                    let fetch_fn = rquickjs::Function::new(ctx.clone(), move |req_json: String| {
                        execute_native_fetch(&http_cl, req_json)
                    })
                    .map_err(|e| format!("QuickJS bind fetch failed: {e}"))?;
                    ctx.globals()
                        .set("__nada_native_fetch", fetch_fn)
                        .map_err(|e| format!("QuickJS set fetch failed: {e}"))?;
                    let mut opts = rquickjs::context::EvalOptions::default();
                    opts.strict = false;
                    ctx.eval_with_options::<(), _>(QUICKJS_POLYFILLS.as_bytes(), opts)
                        .map_err(|e| format!("QuickJS polyfill eval failed: {e}"))?;
                    Ok(())
                });

                if let Err(e) = poly_res {
                    let _ = init_tx.send(Err(e));
                    return;
                }
                let _ = init_tx.send(Ok(()));

                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        QjsCmd::EvalAsync(script_body, timeout, reply) => {
                            let (req_id, cb_url, cb_rx) = bridge.register();
                            let wrapped = wrap_async_script(&script_body, &cb_url);
                            let eval_res: Result<(), String> = ctx.with(|ctx| {
                                let mut opts = rquickjs::context::EvalOptions::default();
                                opts.strict = false;
                                ctx.eval_with_options::<rquickjs::Value<'_>, _>(
                                    wrapped.as_bytes(),
                                    opts,
                                )
                                .map(|_| ())
                                .map_err(|e| format!("QuickJS async eval error: {e}"))
                            });
                            if let Err(e) = eval_res {
                                bridge.cancel(&req_id);
                                let _ = reply.send(Err(e));
                                continue;
                            }
                            let deadline = Instant::now() + timeout;
                            loop {
                                match rt.execute_pending_job() {
                                    Ok(true) => continue,
                                    Ok(false) => {}
                                    Err(_) => {}
                                }
                                if let Ok(raw) = cb_rx.try_recv() {
                                    let _ = reply.send(unwrap_callback_payload(&raw));
                                    break;
                                }
                                if Instant::now() >= deadline {
                                    bridge.cancel(&req_id);
                                    let _ = reply.send(Err(
                                        "QuickJS async callback timed out".to_string(),
                                    ));
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(5));
                            }
                        }
                        QjsCmd::EvalSync(expr, _timeout, reply) => {
                            let wrapped = format!("JSON.stringify({expr})");
                            let res: Result<Value, String> = ctx.with(|ctx| {
                                let mut opts = rquickjs::context::EvalOptions::default();
                                opts.strict = false;
                                let s: String = ctx
                                    .eval_with_options(wrapped.as_bytes(), opts)
                                    .map_err(|e| format!("QuickJS sync eval error: {e}"))?;
                                parse_sync_json_result(&s)
                            });
                            while let Ok(true) = rt.execute_pending_job() {}
                            let _ = reply.send(res);
                        }
                    }
                }
            })
            .map_err(|e| format!("failed to spawn QuickJsHost thread: {e}"))?;

        init_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|e| format!("QuickJsHost init timeout: {e}"))??;

        Ok(Self { tx })
    }
}

impl JsHost for QuickJsHost {
    fn eval_async_json(&self, script_body: &str, timeout: Duration) -> Result<Value, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(QjsCmd::EvalAsync(script_body.to_string(), timeout, reply_tx))
            .map_err(|e| format!("QuickJsHost send failed: {e}"))?;
        reply_rx
            .recv_timeout(timeout + Duration::from_secs(2))
            .map_err(|e| format!("QuickJsHost async recv timeout: {e}"))?
    }

    fn eval_sync_json(&self, expr: &str, timeout: Duration) -> Result<Value, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(QjsCmd::EvalSync(expr.to_string(), timeout, reply_tx))
            .map_err(|e| format!("QuickJsHost send failed: {e}"))?;
        reply_rx
            .recv_timeout(timeout + Duration::from_secs(2))
            .map_err(|e| format!("QuickJsHost sync recv timeout: {e}"))?
    }
}

#[derive(Clone, Debug)]
pub struct VideoContext {
    pub user_agent: String,
    pub visitor_data: String,
    pub content_pot: String,
    pub sts: u64,
}

#[derive(Clone)]
struct PlayerMeta {
    hash: String,
    sts: u64,
    fetched_at: Instant,
}

struct BgState {
    user_agent: Option<String>,
    visitor_data: Option<(String, Instant)>,
    bg_valid_until: Option<Instant>,
    player: Option<PlayerMeta>,
}

pub struct BgEngine {
    host: Arc<dyn JsHost>,
    http: reqwest::Client,
    state: tokio::sync::Mutex<BgState>,
}

pub(crate) fn is_transient_transport_err(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_request() || err.is_timeout()
}

pub(crate) async fn send_with_transient_retry<F>(
    mut make_req: F,
) -> Result<reqwest::Response, reqwest::Error>
where
    F: FnMut() -> reqwest::RequestBuilder,
{
    let mut attempt = 0u64;
    loop {
        match make_req().send().await {
            Ok(resp) => return Ok(resp),
            Err(err) if attempt < 2 && is_transient_transport_err(&err) => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(150 * attempt)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

impl BgEngine {
    pub fn new(host: Arc<dyn JsHost>) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            host,
            http,
            state: tokio::sync::Mutex::new(BgState {
                user_agent: None,
                visitor_data: None,
                bg_valid_until: None,
                player: None,
            }),
        })
    }

    pub fn http_client(&self) -> &reqwest::Client {
        &self.http
    }

    async fn eval_async(&self, script: &str, timeout: Duration) -> Result<Value, String> {
        let host = Arc::clone(&self.host);
        let script = script.to_string();
        tokio::task::spawn_blocking(move || host.eval_async_json(&script, timeout))
            .await
            .map_err(|e| format!("spawn_blocking join error: {e}"))?
    }

    async fn eval_sync(&self, expr: &str, timeout: Duration) -> Result<Value, String> {
        let host = Arc::clone(&self.host);
        let expr = expr.to_string();
        tokio::task::spawn_blocking(move || host.eval_sync_json(&expr, timeout))
            .await
            .map_err(|e| format!("spawn_blocking join error: {e}"))?
    }

    async fn ensure_botguard_locked(&self, st: &mut BgState, force: bool) -> Result<(), String> {
        let valid = !force
            && st
                .bg_valid_until
                .is_some_and(|until| Instant::now() < until);
        if valid && st.user_agent.is_some() {
            return Ok(());
        }

        const BOTGUARD_INIT_JS_TEMPLATE: &str = r#"
const hdrs = {
  "content-type": "application/json+protobuf",
  "x-goog-api-key": __NADA_JNN_KEY__,
  "x-user-agent": "grpc-web-javascript/0.1"
};
const cResp = await fetch("https://www.youtube.com/api/jnn/v1/Create", {
  method: "POST",
  headers: hdrs,
  body: JSON.stringify(["O43z0dpjhgX20SCx4KAo"])
});
if (!cResp.ok) throw new Error("BotGuard Create HTTP " + cResp.status);
const cJson = await cResp.json();
let cdata = cJson[0];
if (cJson.length > 1 && typeof cJson[1] === "string") {
  const raw = atob(cJson[1]);
  const bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) bytes[i] = (raw.charCodeAt(i) + 97) & 0xff;
  cdata = JSON.parse(new TextDecoder().decode(bytes));
}
let interp = (cdata[1] || []).find(Boolean);
if (!interp && cdata[2]) {
  let u = Array.isArray(cdata[2]) ? cdata[2].find(Boolean) : cdata[2];
  if (u && u.startsWith("//")) u = "https:" + u;
  if (u) interp = await (await fetch(u)).text();
}
if (!interp) throw new Error("BotGuard interpreter missing");
new Function(interp)();
const vmObj = globalThis[cdata[5]];
if (!vmObj || typeof vmObj.a !== "function") throw new Error("BotGuard VM global missing");
let vmFns = {};
await vmObj.a(cdata[4], (a, b, c, d) => { vmFns = { a, b, c, d }; }, true, undefined, () => {}, [[], []])[0];
const sigOut = [];
const bgResp = await new Promise(resolve => vmFns.a(resolve, [undefined, undefined, sigOut, undefined]));
const gResp = await fetch("https://www.youtube.com/api/jnn/v1/GenerateIT", {
  method: "POST",
  headers: hdrs,
  body: JSON.stringify(["O43z0dpjhgX20SCx4KAo", bgResp])
});
if (!gResp.ok) throw new Error("BotGuard GenerateIT HTTP " + gResp.status);
const gJson = await gResp.json();
if (!gJson[0] || !sigOut[0]) throw new Error("GenerateIT missing integrityToken or sigOut");
const uw = s => Uint8Array.from(atob(s.replace(/[-_.]/g, c => ({ "-": "+", "_": "/", ".": "=" }[c]))), ch => ch.charCodeAt(0));
window.__nada_minter = await sigOut[0](uw(gJson[0]));
return { ua: navigator.userAgent, ttl: Number(gJson[1] || 43200) };
"#;

        let api_key = resolve_jnn_api_key()?;
        let api_key_json = serde_json::to_string(&api_key)
            .map_err(|e| format!("failed to encode NADA_JNN_KEY: {e}"))?;
        let botguard_init_js = BOTGUARD_INIT_JS_TEMPLATE.replace("__NADA_JNN_KEY__", &api_key_json);

        let val = self
            .eval_async(&botguard_init_js, Duration::from_secs(20))
            .await?;
        let ua = val
            .get("ua")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(BG_USER_AGENT_FALLBACK)
            .to_string();
        let ttl_secs = val
            .get("ttl")
            .and_then(Value::as_u64)
            .unwrap_or(43200)
            .saturating_sub(600)
            .max(60);
        st.user_agent = Some(ua);
        st.bg_valid_until = Some(Instant::now() + Duration::from_secs(ttl_secs));
        Ok(())
    }

    async fn ensure_visitor_data_locked(&self, st: &mut BgState) -> Result<String, String> {
        if let Some((vd, fetched)) = &st.visitor_data {
            if fetched.elapsed() < VISITOR_TTL && !vd.is_empty() {
                return Ok(vd.clone());
            }
        }
        let ua = st
            .user_agent
            .as_deref()
            .unwrap_or(BG_USER_AGENT_FALLBACK);
        let payload = serde_json::json!({
            "context": {
                "client": {
                    "clientName": "WEB",
                    "clientVersion": "2.20261123.08.00",
                    "hl": "en",
                    "gl": "ID"
                }
            }
        });
        let resp = send_with_transient_retry(|| {
            self.http
                .post("https://www.youtube.com/youtubei/v1/visitor_id?prettyPrint=false")
                .header(reqwest::header::USER_AGENT, ua)
                .header(reqwest::header::ORIGIN, "https://www.youtube.com")
                .header(reqwest::header::REFERER, "https://www.youtube.com/")
                .json(&payload)
        })
        .await
        .map_err(|e| format!("visitor_id request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("visitor_id HTTP {}", resp.status()));
        }
        let json: Value = resp
            .json()
            .await
            .map_err(|e| format!("visitor_id json parse failed: {e}"))?;
        let vd = json
            .pointer("/responseContext/visitorData")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "visitor_id response missing visitorData".to_string())?
            .to_string();
        st.visitor_data = Some((vd.clone(), Instant::now()));
        Ok(vd)
    }

    async fn ensure_player_locked(&self, st: &mut BgState) -> Result<u64, String> {
        if let Some(meta) = &st.player {
            if meta.fetched_at.elapsed() < PLAYER_TTL {
                return Ok(meta.sts);
            }
        }
        let ua = st
            .user_agent
            .as_deref()
            .unwrap_or(BG_USER_AGENT_FALLBACK);
        let iframe_resp = send_with_transient_retry(|| {
            self.http
                .get("https://www.youtube.com/iframe_api")
                .header(reqwest::header::USER_AGENT, ua)
        })
        .await
        .map_err(|e| format!("iframe_api fetch failed: {e}"))?;
        if !iframe_resp.status().is_success() {
            return Err(format!("iframe_api HTTP {}", iframe_resp.status()));
        }
        let iframe_js = iframe_resp
            .text()
            .await
            .map_err(|e| format!("iframe_api body failed: {e}"))?;
        let hash = extract_player_hash(&iframe_js)
            .ok_or_else(|| "failed to parse player hash from iframe_api".to_string())?;

        if let Some(meta) = &st.player {
            if meta.hash == hash {
                let sts = meta.sts;
                st.player = Some(PlayerMeta {
                    hash,
                    sts,
                    fetched_at: Instant::now(),
                });
                return Ok(sts);
            }
        }

        let base_url = format!(
            "https://www.youtube.com/s/player/{}/player_ias.vflset/en_US/base.js",
            hash
        );
        let base_resp = send_with_transient_retry(|| {
            self.http
                .get(&base_url)
                .header(reqwest::header::USER_AGENT, ua)
        })
        .await
        .map_err(|e| format!("base.js fetch failed: {e}"))?;
        if !base_resp.status().is_success() {
            return Err(format!("base.js HTTP {}", base_resp.status()));
        }
        let base_js = base_resp
            .text()
            .await
            .map_err(|e| format!("base.js body failed: {e}"))?;

        let (patched, sts) = patch_base_js(&base_js)?;
        let patched_json =
            serde_json::to_string(&patched).map_err(|e| format!("base.js encode error: {e}"))?;
        let eval_script = format!(
            "(0, eval)({patched_json}); return {{ ready: Boolean(window._yt_player && typeof window._yt_player.__nada_deobf === 'function') }};"
        );
        let res = self
            .eval_async(&eval_script, Duration::from_secs(20))
            .await?;
        let ready = res.get("ready").and_then(Value::as_bool).unwrap_or(false);
        if !ready {
            return Err("base.js evaluated but __nada_deobf was not installed".to_string());
        }
        st.player = Some(PlayerMeta {
            hash,
            sts,
            fetched_at: Instant::now(),
        });
        Ok(sts)
    }

    async fn mint_pot_locked(&self, ident: &str) -> Result<String, String> {
        let ident_json =
            serde_json::to_string(ident).map_err(|e| format!("ident encode error: {e}"))?;
        let script = format!(
            r#"
if (typeof window.__nada_minter !== "function") throw new Error("missing __nada_minter");
const bytes = await window.__nada_minter(new TextEncoder().encode({ident_json}));
let s = "";
for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
const pot = btoa(s).replace(/\+/g, "-").replace(/\//g, "_");
return {{ pot }};
"#
        );
        let val = self.eval_async(&script, Duration::from_secs(10)).await?;
        val.get("pot")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .ok_or_else(|| "mint_pot returned empty pot".to_string())
    }

    pub async fn ensure_initialized(&self) -> Result<(), String> {
        let mut st = self.state.lock().await;
        self.ensure_botguard_locked(&mut st, false).await?;
        self.ensure_visitor_data_locked(&mut st).await?;
        self.ensure_player_locked(&mut st).await?;
        Ok(())
    }

    pub async fn prepare_for_video(&self, video_id: &str) -> Result<VideoContext, String> {
        let mut st = self.state.lock().await;
        self.ensure_botguard_locked(&mut st, false).await?;
        let visitor_data = self.ensure_visitor_data_locked(&mut st).await?;
        let sts = self.ensure_player_locked(&mut st).await?;
        let content_pot = match self.mint_pot_locked(video_id).await {
            Ok(p) => p,
            Err(_) => {
                self.ensure_botguard_locked(&mut st, true).await?;
                self.mint_pot_locked(video_id).await?
            }
        };
        let user_agent = st
            .user_agent
            .clone()
            .unwrap_or_else(|| BG_USER_AGENT_FALLBACK.to_string());
        Ok(VideoContext {
            user_agent,
            visitor_data,
            content_pot,
            sts,
        })
    }

    pub async fn deobf_batch(&self, items: &[(String, String, String)]) -> Result<Vec<String>, String> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let items_json =
            serde_json::to_string(items).map_err(|e| format!("deobf_batch encode error: {e}"))?;
        let expr = format!(
            r#"(() => {{
  const items = {items_json};
  const fn = window._yt_player && window._yt_player.__nada_deobf;
  if (typeof fn !== "function") return {{ error: "missing __nada_deobf" }};
  try {{
    return {{ urls: items.map(([u, sp, s]) => fn(u, sp, s)) }};
  }} catch (e) {{
    return {{ error: String(e && (e.message || e)) }};
  }}
}})()"#
        );
        let mut st = self.state.lock().await;
        let mut val = self.eval_sync(&expr, Duration::from_secs(10)).await?;
        if val.get("error").and_then(Value::as_str) == Some("missing __nada_deobf") {
            st.player = None;
            self.ensure_player_locked(&mut st).await?;
            val = self.eval_sync(&expr, Duration::from_secs(10)).await?;
        }
        if let Some(err) = val.get("error").and_then(Value::as_str) {
            return Err(err.to_string());
        }
        let urls = val
            .get("urls")
            .and_then(Value::as_array)
            .ok_or_else(|| "deobf_batch missing urls array".to_string())?;
        let mut out = Vec::with_capacity(urls.len());
        for u in urls {
            let s = u
                .as_str()
                .ok_or_else(|| "deobf_batch non-string url".to_string())?;
            out.push(s.to_string());
        }
        if out.len() != items.len() {
            return Err("deobf_batch length mismatch".to_string());
        }
        Ok(out)
    }
}

pub fn extract_player_hash(iframe_api: &str) -> Option<String> {
    let normalized = iframe_api.replace(r"\/", "/");
    let needle = "/s/player/";
    let pos = normalized.find(needle)?;
    let rest = &normalized[pos + needle.len()..];
    let end = rest.find('/')?;
    let hash = &rest[..end];
    if hash.is_empty() {
        None
    } else {
        Some(hash.to_string())
    }
}

fn floor_char_boundary(s: &str, idx: usize) -> usize {
    let mut pos = idx.min(s.len());
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 1usize;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    for (i, b) in s.bytes().enumerate() {
        if let Some(q) = quote {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == q {
                quote = None;
            }
            continue;
        }
        match b {
            b'\'' | b'"' => quote = Some(b),
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    let mut start = 0usize;

    for (i, b) in args.bytes().enumerate() {
        if let Some(q) = quote {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == q {
                quote = None;
            }
            continue;
        }
        match b {
            b'\'' | b'"' => quote = Some(b),
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            b'{' => brace_depth += 1,
            b'}' => brace_depth = brace_depth.saturating_sub(1),
            b',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                out.push(args[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(args[start..].trim().to_string());
    out
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b == b'.'
}

fn extract_sig_fn(js: &str) -> Result<String, String> {
    let alr_pos = js
        .find("a.set(\"alr\",\"yes\")")
        .or_else(|| js.find("a.set('alr','yes')"))
        .or_else(|| {
            for pat in [".set(\"alr\",\"yes\")", ".set('alr','yes')"] {
                let mut start = 0;
                while let Some(rel) = js[start..].find(pat) {
                    let abs = start + rel;
                    let prefix_start = floor_char_boundary(js, abs.saturating_sub(30));
                    let prefix = &js[prefix_start..abs];
                    if !prefix.contains("||") {
                        return Some(abs);
                    }
                    start = abs + pat.len();
                }
            }
            None
        })
        .ok_or_else(|| "patch_base_js: missing .set(\"alr\",\"yes\")".to_string())?;
    let search_start = floor_char_boundary(js, alr_pos.saturating_sub(450));
    let window = &js[search_start..alr_pos];
    let fn_rel = window
        .rfind("=function(")
        .ok_or_else(|| "patch_base_js: missing =function( before alr".to_string())?;
    let fn_abs = search_start + fn_rel;
    let bytes = js.as_bytes();
    let mut start = fn_abs;
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }
    let name = &js[start..fn_abs];
    if name.is_empty() {
        return Err("patch_base_js: empty sig_fn name".to_string());
    }
    Ok(name.to_string())
}

fn extract_n_call(js: &str) -> Result<String, String> {
    let get_pos = js
        .find(".get(\"alr\")||")
        .or_else(|| js.find(".get('alr')||"))
        .ok_or_else(|| "patch_base_js: missing .get(\"alr\")||".to_string())?;
    let search_start = floor_char_boundary(js, get_pos.saturating_sub(250));
    let window = &js[search_start..get_pos];

    // Prefer pattern `b=c?FN(args):fallback;`
    let mut scan = window;
    while let Some(q_pos) = scan.rfind('?') {
        let after_q = &scan[q_pos + 1..];
        if let Some(paren_open) = after_q.find('(') {
            let fn_name = after_q[..paren_open].trim();
            if !fn_name.is_empty() && fn_name.bytes().all(is_ident_char) {
                let after_open = &after_q[paren_open + 1..];
                if let Some(paren_close) = find_matching_paren(after_open) {
                    let args_str = &after_open[..paren_close];
                    let after_close = &after_open[paren_close + 1..];
                    if let Some(colon_pos) = after_close.find(':') {
                        let fallback_end = after_close[colon_pos + 1..]
                            .find(';')
                            .unwrap_or(after_close.len() - colon_pos - 1);
                        let fallback = after_close[colon_pos + 1..colon_pos + 1 + fallback_end].trim();
                        let mut parts = split_top_level_args(args_str);
                        let mut replaced_url = false;
                        for p in &mut parts {
                            if !fallback.is_empty() && p == fallback {
                                *p = "x".to_string();
                                replaced_url = true;
                            }
                        }
                        if !replaced_url {
                            if parts.len() >= 3 {
                                parts[2] = "x".to_string();
                            } else if !parts.is_empty() {
                                let last = parts.len() - 1;
                                parts[last] = "x".to_string();
                            }
                        }
                        if parts.len() >= 4 {
                            parts[3] = "false".to_string();
                        }
                        return Ok(format!("{fn_name}({})", parts.join(",")));
                    }
                }
            }
        }
        scan = &scan[..q_pos];
    }
    Err("patch_base_js: missing n-parameter call before .get(\"alr\")||".to_string())
}

pub fn extract_signature_timestamp(js: &str) -> Result<u64, String> {
    for needle in ["signatureTimestamp:", "signatureTimestamp="] {
        if let Some(pos) = js.find(needle) {
            let rest = &js[pos + needle.len()..];
            let digits: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(v) = digits.parse::<u64>() {
                if v > 0 {
                    return Ok(v);
                }
            }
        }
    }
    Err("patch_base_js: missing signatureTimestamp".to_string())
}

pub fn patch_base_js(js: &str) -> Result<(String, u64), String> {
    let sig_fn = extract_sig_fn(js)?;
    let n_call = extract_n_call(js)?;
    let sts = extract_signature_timestamp(js)?;

    let (insert_at, suffix_str) = if let Some(idx) = js.rfind("})(_yt_player);") {
        (idx, "})(_yt_player);")
    } else if let Some(idx) = js.rfind("})(_yt_player)") {
        (idx, "})(_yt_player)")
    } else {
        return Err("patch_base_js: missing closing })(_yt_player)".to_string());
    };

    let hook = format!(
        ";g.__nada_deobf=function(u,sp,s){{var x={sig_fn}(u,sp||\"\",s||\"\");var r={n_call};if(typeof r===\"string\")return r;if(r&&typeof r===\"object\"){{if(typeof r.url===\"string\"&&r.url.startsWith(\"http\"))return r.url;var proto=Object.getPrototypeOf(r);if(proto){{for(var k of Object.getOwnPropertyNames(proto)){{if(k!==\"constructor\"&&typeof r[k]===\"function\"&&r[k].length===0){{try{{var v=r[k]();if(typeof v===\"string\"&&v.startsWith(\"http\"))return v;}}catch(_){{}}}}}}}}}}return String(r||x);}};"
    );

    let mut out = String::with_capacity(js.len() + hook.len() + 8);
    out.push_str(&js[..insert_at]);
    out.push_str(&hook);
    out.push_str(suffix_str);
    out.push_str(&js[insert_at + suffix_str.len()..]);
    Ok((out, sts))
}

fn read_jnn_key_from_env_file(path: &std::path::Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("NADA_JNN_KEY=") {
            let val = rest.trim().trim_matches('"').trim_matches('\'');
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn resolve_jnn_api_key() -> Result<String, String> {
    if let Ok(val) = std::env::var("NADA_JNN_KEY") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    for candidate in [".env", "../.env"] {
        if let Some(val) = read_jnn_key_from_env_file(std::path::Path::new(candidate)) {
            return Ok(val);
        }
    }
    #[cfg(debug_assertions)]
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        if let Some(repo_env) = std::path::Path::new(manifest_dir)
            .parent()
            .map(|p| p.join(".env"))
        {
            if let Some(val) = read_jnn_key_from_env_file(&repo_env) {
                return Ok(val);
            }
        }
    }
    if let Some(val) = option_env!("NADA_JNN_KEY") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    Err("NADA_JNN_KEY is not set (configure it in .env or environment)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_time_jnn_key_uses_environment_then_repo_env() {
        let repo_env = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri has a parent")
            .join(".env");
        let expected = std::env::var("NADA_JNN_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| read_jnn_key_from_env_file(&repo_env));
        let Some(expected) = expected else {
            return;
        };
        assert!(
            option_env!("NADA_JNN_KEY") == Some(expected.as_str()),
            "release builds must embed the configured BotGuard key"
        );
    }

    #[test]
    fn extract_player_hash_parses_iframe_api() {
        let sample = r#"var scriptUrl = 'https:\/\/www.youtube.com\/s\/player\/b485e332\/www-widgetapi.vflset\/www-widgetapi.js';"#;
        assert_eq!(extract_player_hash(sample).as_deref(), Some("b485e332"));
    }

    #[test]
    fn patch_base_js_extracts_sig_n_and_sts() {
        let sample = r#"
var _yt_player={};(function(g){var window=this;
var config={signatureTimestamp:20779};
$o=function(a,b,c){b=b===void 0?"":b;c=c===void 0?"":c;a=new g.Xo(a,!0);a.set("alr","yes");c&&(c=Yo(2,7910,Zo(18,237,c)),a[ca[20]](b,se(8,5744,c)));return a};
g.eO=function(a,b,c){if(a.j){b=c?Er(18,8580,a.j,b.jx):a.j;var d=new jG(b);d.get("alr")||d.set("alr","yes");}};
})(_yt_player);
"#;
        let (patched, sts) = patch_base_js(sample).unwrap();
        assert_eq!(sts, 20779);
        assert!(patched.contains("g.__nada_deobf=function(u,sp,s)"));
        assert!(patched.contains("var x=$o(u,sp||\"\",s||\"\");"));
        assert!(patched.contains("var r=Er(18,8580,x,false);"));
        assert!(patched.ends_with("})(_yt_player);\n"));
    }

    #[test]
    fn patch_base_js_handles_3_arg_n_and_single_quotes() {
        let sample = r#"
var _yt_player={};(function(g){
var x={signatureTimestamp=20801};
g.ab=function(p,q,r){p=new g.Xo(p,!0);p.set('alr','yes');return p};
if(m.url){b=c?Nf(7,m.url):m.url;var d=new jG(b);d.get('alr')||d.set('alr','yes');}
})(_yt_player)
"#;
        let (patched, sts) = patch_base_js(sample).unwrap();
        assert_eq!(sts, 20801);
        assert!(patched.contains("var x=g.ab(u,sp||\"\",s||\"\");"));
        assert!(patched.contains("var r=Nf(7,x);"));
    }

    #[test]
    fn patch_base_js_handles_utf8_boundaries_and_nested_parens() {
        let pad_428 = "a".repeat(428);
        let pad_185 = "b".repeat(185);
        // Place multi-byte UTF-8 chars right across the 450-byte / 250-byte slice edges
        let sample = format!(
            "var _yt_player={{}};(function(g){{\n\
            var signatureTimestamp=20999;\n\
            var u='🎵';$x=function(a,b,c){{{pad_428}a.set(\"alr\",\"yes\");return a}};\n\
            var m='🎵';{pad_185}if(a.j){{b=c?Er(18,helper(1,2),a.j,b.jx):a.j;var d=new jG(b);d.get(\"alr\")||d.set(\"alr\",\"yes\");}}\n\
            }})(_yt_player);"
        );
        let (patched, sts) = patch_base_js(&sample).unwrap();
        assert_eq!(sts, 20999);
        assert!(patched.contains("var x=$x(u,sp||\"\",s||\"\");"));
        assert!(patched.contains("var r=Er(18,helper(1,2),x,false);"));
    }

    #[test]
    fn send_with_transient_retry_recovers_after_connection_drop() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::AtomicUsize;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_bg = hits.clone();

        std::thread::spawn(move || {
            for stream in listener.incoming().take(3) {
                let Ok(mut s) = stream else { break };
                let n = hits_bg.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                if n < 2 {
                    // Drop connection without sending HTTP response (transient error)
                    drop(s);
                } else {
                    let _ = s.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    );
                }
            }
        });

        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let url = format!("http://127.0.0.1:{port}/test");
        let resp = rt
            .block_on(send_with_transient_retry(|| client.get(&url)))
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn callback_bridge_roundtrip() {
        let bridge = CallbackBridge::start().unwrap();
        let (_id, url, rx) = bridge.register();
        let client = reqwest::blocking::Client::new();
        let resp = client
            .post(&url)
            .body(r#"{"ok":true,"value":{"pot":"abc123"}}"#)
            .send()
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let raw = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let val = unwrap_callback_payload(&raw).unwrap();
        assert_eq!(val["pot"], "abc123");
    }
}
