//! Resale Inventory server: one SQLite file, one web page. Used by the standalone binary and the desktop app.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post, put},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, UdpSocket};
use std::sync::{Arc, Mutex};

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS items (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    sku TEXT NOT NULL DEFAULT '',
    cost REAL NOT NULL DEFAULT 0,
    price REAL NOT NULL DEFAULT 0,
    platform TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'Unlisted',
    url TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT '',
    photo BLOB,
    updated TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS sessions (token TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

// Status-driven dates: listed_at is kept while Listed/Sold, sold_at and sold_price only while Sold.
const DATES: &str = "
    listed_at = CASE WHEN ?6 IN ('Listed', 'Sold') THEN COALESCE(listed_at, CURRENT_DATE) END,
    sold_at = CASE WHEN ?6 = 'Sold' THEN COALESCE(sold_at, CURRENT_DATE) END,
    sold_price = CASE WHEN ?6 = 'Sold' THEN ?9 END";

struct App {
    db: Mutex<Connection>,
    /// Fixed password from RI_PASSWORD; when None the password is set up in the browser and stored hashed.
    env_password: Option<String>,
    /// Port to show phones, when listening on all network interfaces.
    lan_port: Option<u16>,
}
type S = State<Arc<App>>;
type R<T> = Result<T, StatusCode>;

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(default)]
struct Item {
    id: i64,
    name: String,
    sku: String,
    cost: f64,
    price: f64,
    platform: String,
    status: String,
    url: String,
    notes: String,
    sold_price: f64,
    listed_at: String,
    sold_at: String,
    has_photo: bool,
    updated: String,
}

#[derive(Deserialize)]
struct Password {
    password: String,
}

fn open(path: &std::path::Path) -> Connection {
    let db = Connection::open(path).expect("cannot open database file");
    db.execute_batch(SCHEMA).expect("cannot create tables");
    // Columns added after v0.1; the error on an already-migrated database is expected.
    for col in ["sold_price REAL", "listed_at TEXT", "sold_at TEXT"] {
        let _ = db.execute(&format!("ALTER TABLE items ADD COLUMN {col}"), []);
    }
    db
}

fn err<E>(_: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Compares in constant time so the password can't be guessed by timing.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn stored_hash(db: &Connection) -> rusqlite::Result<Option<String>> {
    db.query_row(
        "SELECT value FROM settings WHERE key = 'password'",
        [],
        |r| r.get(0),
    )
    .optional()
}

fn hash(password: &str) -> String {
    use argon2::PasswordHasher;
    argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .expect("hashing failed")
        .to_string()
}

fn verify(password: &str, hash: &str) -> bool {
    use argon2::{PasswordHash, PasswordVerifier};
    PasswordHash::new(hash).is_ok_and(|h| {
        argon2::Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

fn guard(app: &App, h: &HeaderMap) -> R<()> {
    let token = h
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .and_then(|c| c.split("; ").find_map(|kv| kv.strip_prefix("ri_session=")))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let db = app.db.lock().map_err(err)?;
    db.query_row(
        "SELECT 1 FROM sessions WHERE token = ?",
        [token],
        |_| Ok(()),
    )
    .optional()
    .map_err(err)?
    .ok_or(StatusCode::UNAUTHORIZED)
}

fn new_session(db: &Connection) -> R<Response> {
    let token: String = db
        .query_row("SELECT hex(randomblob(32))", [], |r| r.get(0))
        .map_err(err)?;
    db.execute("INSERT INTO sessions (token) VALUES (?)", [&token])
        .map_err(err)?;
    let cookie = format!("ri_session={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000");
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

/// Tells the page whether to show "create a password" or "log in".
async fn status(State(app): S) -> R<Json<serde_json::Value>> {
    let db = app.db.lock().map_err(err)?;
    let setup = app.env_password.is_none() && stored_hash(&db).map_err(err)?.is_none();
    Ok(Json(serde_json::json!({ "setup": setup })))
}

async fn setup(State(app): S, Json(body): Json<Password>) -> R<Response> {
    if body.password.chars().count() < 8 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = app.db.lock().map_err(err)?;
    // Only the very first visitor can set the password.
    if app.env_password.is_some() || stored_hash(&db).map_err(err)?.is_some() {
        return Err(StatusCode::CONFLICT);
    }
    db.execute(
        "INSERT INTO settings (key, value) VALUES ('password', ?)",
        [hash(&body.password)],
    )
    .map_err(err)?;
    new_session(&db)
}

async fn login(State(app): S, Json(body): Json<Password>) -> R<Response> {
    let ok = match &app.env_password {
        Some(p) => same(&body.password, p),
        None => {
            let db = app.db.lock().map_err(err)?;
            let h = stored_hash(&db).map_err(err)?;
            h.is_some_and(|h| verify(&body.password, &h))
        }
    };
    if !ok {
        // Slows down password guessing.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        return Err(StatusCode::UNAUTHORIZED);
    }
    let db = app.db.lock().map_err(err)?;
    new_session(&db)
}

fn list_items(db: &Connection) -> rusqlite::Result<Vec<Item>> {
    db.prepare(
        "SELECT id, name, sku, cost, price, platform, status, url, notes, COALESCE(sold_price, price),
                COALESCE(listed_at, ''), COALESCE(sold_at, ''), photo IS NOT NULL, updated
         FROM items ORDER BY updated DESC, id DESC",
    )?
    .query_map([], |r| {
        Ok(Item {
            id: r.get(0)?,
            name: r.get(1)?,
            sku: r.get(2)?,
            cost: r.get(3)?,
            price: r.get(4)?,
            platform: r.get(5)?,
            status: r.get(6)?,
            url: r.get(7)?,
            notes: r.get(8)?,
            sold_price: r.get(9)?,
            listed_at: r.get(10)?,
            sold_at: r.get(11)?,
            has_photo: r.get(12)?,
            updated: r.get(13)?,
        })
    })?
    .collect()
}

fn save_item(db: &Connection, id: Option<i64>, i: &Item) -> rusqlite::Result<i64> {
    let id = match id {
        Some(id) => id,
        None => {
            db.execute("INSERT INTO items (name) VALUES (?)", [&i.name])?;
            db.last_insert_rowid()
        }
    };
    db.execute(
        &format!(
            "UPDATE items SET name=?1, sku=?2, cost=?3, price=?4, platform=?5, status=?6, url=?7,
             notes=?8, updated=CURRENT_TIMESTAMP, {DATES} WHERE id=?10"
        ),
        params![
            i.name,
            i.sku,
            i.cost,
            i.price,
            i.platform,
            i.status,
            i.url,
            i.notes,
            i.sold_price,
            id
        ],
    )?;
    Ok(id)
}

async fn list(State(app): S, h: HeaderMap) -> R<Json<Vec<Item>>> {
    guard(&app, &h)?;
    let db = app.db.lock().map_err(err)?;
    list_items(&db).map(Json).map_err(err)
}

async fn create(State(app): S, h: HeaderMap, Json(item): Json<Item>) -> R<Json<i64>> {
    guard(&app, &h)?;
    if item.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = app.db.lock().map_err(err)?;
    save_item(&db, None, &item).map(Json).map_err(err)
}

async fn update(
    State(app): S,
    h: HeaderMap,
    Path(id): Path<i64>,
    Json(item): Json<Item>,
) -> R<Json<i64>> {
    guard(&app, &h)?;
    if item.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = app.db.lock().map_err(err)?;
    save_item(&db, Some(id), &item).map(Json).map_err(err)
}

async fn delete(State(app): S, h: HeaderMap, Path(id): Path<i64>) -> R<StatusCode> {
    guard(&app, &h)?;
    let db = app.db.lock().map_err(err)?;
    db.execute("DELETE FROM items WHERE id = ?", [id])
        .map_err(err)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn put_photo(State(app): S, h: HeaderMap, Path(id): Path<i64>, body: Bytes) -> R<StatusCode> {
    guard(&app, &h)?;
    // The page always uploads a resized JPEG; reject anything else so we never serve foreign content.
    if !body.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let db = app.db.lock().map_err(err)?;
    db.execute(
        "UPDATE items SET photo = ?, updated = CURRENT_TIMESTAMP WHERE id = ?",
        params![&body[..], id],
    )
    .map_err(err)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_photo(State(app): S, h: HeaderMap, Path(id): Path<i64>) -> R<Response> {
    guard(&app, &h)?;
    let db = app.db.lock().map_err(err)?;
    let photo: Vec<u8> = db
        .query_row(
            "SELECT photo FROM items WHERE id = ? AND photo IS NOT NULL",
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, "private, max-age=31536000"),
        ],
        photo,
    )
        .into_response())
}

/// This computer's address on the home network (no packet is sent; it only asks the OS for a route).
fn lan_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.0.1:80").ok()?;
    Some(s.local_addr().ok()?.ip())
}

/// The address a phone on the same Wi-Fi should open, if this server is reachable from the network.
async fn phone_url(State(app): S, h: HeaderMap) -> R<Json<Option<String>>> {
    guard(&app, &h)?;
    Ok(Json(
        app.lan_port
            .zip(lan_ip())
            .map(|(port, ip)| format!("http://{ip}:{port}")),
    ))
}

#[derive(Deserialize)]
struct QrQuery {
    text: String,
}

async fn qr(State(app): S, h: HeaderMap, Query(q): Query<QrQuery>) -> R<Response> {
    guard(&app, &h)?;
    let code = qrcode::QrCode::new(q.text.as_bytes()).map_err(|_| StatusCode::BAD_REQUEST)?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .build();
    Ok(([(header::CONTENT_TYPE, "image/svg+xml")], svg).into_response())
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(|| async { Html(include_str!("index.html")) }))
        .route(
            "/manifest.json",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "application/manifest+json"),
                        // Public file; lets the desktop app check it found a real server.
                        (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                    ],
                    include_str!("manifest.json"),
                )
            }),
        )
        .route("/api/status", get(status))
        .route("/api/setup", post(setup))
        .route("/api/login", post(login))
        .route("/api/phone", get(phone_url))
        .route("/api/qr", get(qr))
        .route("/api/items", get(list).post(create))
        .route("/api/items/{id}", put(update).delete(delete))
        .route("/api/items/{id}/photo", put(put_photo).get(get_photo))
        .layer(axum::extract::DefaultBodyLimit::max(5 * 1024 * 1024))
        .with_state(app)
}

/// Serves Resale Inventory on `listener` until the process exits.
pub async fn serve(
    listener: std::net::TcpListener,
    db_path: std::path::PathBuf,
    env_password: Option<String>,
) {
    let addr = listener.local_addr().expect("listener has an address");
    let app = Arc::new(App {
        db: Mutex::new(open(&db_path)),
        env_password,
        lan_port: addr.ip().is_unspecified().then_some(addr.port()),
    });
    listener
        .set_nonblocking(true)
        .expect("cannot configure listener");
    let listener = tokio::net::TcpListener::from_std(listener).expect("cannot start listener");
    axum::serve(listener, router(app))
        .await
        .expect("server stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_update_list() {
        let db = open(std::path::Path::new(":memory:"));
        let mut item = Item {
            name: "Nike hoodie".into(),
            cost: 4.0,
            price: 25.0,
            status: "Listed".into(),
            ..Default::default()
        };
        let id = save_item(&db, None, &item).unwrap();
        assert!(!list_items(&db).unwrap()[0].listed_at.is_empty());

        item.status = "Sold".into();
        item.sold_price = 20.0;
        save_item(&db, Some(id), &item).unwrap();
        let got = &list_items(&db).unwrap()[0];
        assert_eq!((got.status.as_str(), got.sold_price), ("Sold", 20.0));
        assert!(!got.sold_at.is_empty() && !got.has_photo);

        item.status = "Unlisted".into();
        save_item(&db, Some(id), &item).unwrap();
        let got = &list_items(&db).unwrap()[0];
        assert!(got.sold_at.is_empty() && got.listed_at.is_empty());
        assert_eq!(got.sold_price, 25.0, "falls back to the asking price");
    }

    #[test]
    fn passwords() {
        assert!(same("hunter2", "hunter2"));
        assert!(!same("hunter2", "hunter22"));
        let h = hash("correct horse");
        assert!(verify("correct horse", &h));
        assert!(!verify("wrong horse", &h));
    }
}
