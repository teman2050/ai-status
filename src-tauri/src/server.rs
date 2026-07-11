use crate::store::Store;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tiny_http::{Header, Method, Request, Response, Server};

pub(crate) type Resp = Response<std::io::Cursor<Vec<u8>>>;

pub(crate) fn json_response(status: u16, body: String) -> Resp {
    let mut resp = Response::from_string(body).with_status_code(status);
    for (key, value) in [
        ("Content-Type", "application/json"),
        ("Access-Control-Allow-Origin", "*"),
        ("Access-Control-Allow-Methods", "GET, POST, OPTIONS"),
        ("Access-Control-Allow-Headers", "Content-Type"),
    ] {
        resp.add_header(Header::from_bytes(key.as_bytes(), value.as_bytes()).unwrap());
    }
    resp
}

fn handle(req: &mut Request, store: &Arc<Mutex<Store>>) -> Resp {
    let method = req.method().clone();
    let path = req.url().split('?').next().unwrap_or("").to_string();
    if method == Method::Options {
        return json_response(204, String::new());
    }
    let now = Instant::now();
    match (method, path.as_str()) {
        (Method::Post, "/api/events") => {
            let mut body = String::new();
            if req.as_reader().read_to_string(&mut body).is_err() {
                return json_response(400, r#"{"ok":false,"error":"unreadable body"}"#.to_string());
            }
            match serde_json::from_str::<crate::store::IncomingEvent>(&body) {
                Ok(event) => {
                    let mut s = store.lock().unwrap();
                    s.apply(event);
                    s.purge(now);
                    json_response(200, r#"{"ok":true}"#.to_string())
                }
                Err(e) => json_response(
                    400,
                    serde_json::json!({"ok": false, "error": e.to_string()}).to_string(),
                ),
            }
        }
        (Method::Get, "/api/tools") => {
            let mut s = store.lock().unwrap();
            s.purge(now);
            json_response(
                200,
                serde_json::json!({"tools": s.tools_snapshot(), "network": s.network()})
                    .to_string(),
            )
        }
        (Method::Get, "/api/tasks") => {
            let mut s = store.lock().unwrap();
            s.refresh_liveness();
            s.purge(now);
            json_response(
                200,
                serde_json::json!({"tasks": s.tasks_snapshot(now)}).to_string(),
            )
        }
        _ => json_response(404, r#"{"ok":false,"error":"not found"}"#.to_string()),
    }
}

pub fn start(store: Arc<Mutex<Store>>, port: u16) {
    std::thread::spawn(move || {
        let server = match Server::http(("127.0.0.1", port)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[ai-status] HTTP server failed to start (port {port}): {e}");
                return;
            }
        };
        for mut request in server.incoming_requests() {
            let resp = handle(&mut request, &store);
            let _ = request.respond(resp);
        }
    });
}
