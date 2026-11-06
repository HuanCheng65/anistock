//! The built frontend (`web/dist`): `/assets/*` (404 for anything outside the folder or missing; 304 on
//! a matching `If-None-Match`), and every other GET through the catch-all: an existing file inside
//! `web/dist` -> that file, else `index.html`. API paths never reach here (`api::fallback`).

use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, Response, StatusCode};
use md5::{Digest, Md5};

/// A plain-text status response (static files only; the API answers JSON).
pub fn plain(status: u16, text: &'static str) -> Response<Body> {
    let mut r = Response::new(Body::from(text));
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    r.headers_mut().insert("content-type", HeaderValue::from_static("text/plain; charset=utf-8"));
    r
}

/// `mimetypes.guess_type` for what a Vite build contains; Starlette adds `charset=utf-8` to `text/*`.
fn content_type(p: &Path) -> String {
    let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let t = match ext.as_str() {
        "html" | "htm" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "xml" => "text/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/vnd.microsoft.icon",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        _ => "text/plain",
    };
    if t.starts_with("text/") {
        format!("{t}; charset=utf-8")
    } else {
        t.to_string()
    }
}

/// Starlette `FileResponse` headers: etag = md5(f"{st_mtime}-{st_size}"), last-modified, length.
async fn file_response(path: &Path, if_none_match: Option<&str>) -> Response<Body> {
    let (meta, bytes) = match (tokio::fs::metadata(path).await, tokio::fs::read(path).await) {
        (Ok(m), Ok(b)) => (m, b),
        _ => return plain(500, "Internal Server Error"),
    };
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).unwrap_or_default();
    // CPython st_mtime: (double)sec + nsec * 1e-9
    let mtime_f = mtime.as_secs() as f64 + mtime.subsec_nanos() as f64 * 1e-9;
    let etag = format!("\"{}\"", hex::encode(Md5::digest(format!("{}-{}", crate::pyfmt::repr(mtime_f), meta.len()))));
    let not_modified = if_none_match.is_some_and(|h| {
        h.split(',').map(|t| t.trim().trim_start_matches("W/")).any(|t| t == etag)
    });
    let mut r = if not_modified {
        let mut r = Response::new(Body::empty());
        *r.status_mut() = StatusCode::NOT_MODIFIED;
        r
    } else {
        let mut r = Response::new(Body::from(bytes));
        let h = r.headers_mut();
        h.insert("content-type", HeaderValue::from_str(&content_type(path)).unwrap());
        h.insert("accept-ranges", HeaderValue::from_static("bytes"));
        h.insert("content-length", HeaderValue::from(meta.len()));
        r
    };
    let h = r.headers_mut();
    // Starlette's NotModifiedResponse keeps only cache-control, content-location, date, etag, expires, vary
    if !not_modified {
        if let Ok(v) = HeaderValue::from_str(&httpdate::fmt_http_date(UNIX_EPOCH + std::time::Duration::from_secs(mtime.as_secs()))) {
            h.insert("last-modified", v);
        }
    }
    h.insert("etag", HeaderValue::from_str(&etag).unwrap());
    r
}

/// `os.path.normpath`, lexically (a leading `..` is kept).
fn normpath(parts: &str) -> PathBuf {
    let mut out: Vec<&str> = Vec::new();
    for c in Path::new(parts).components() {
        match c {
            Component::Normal(s) => out.push(s.to_str().unwrap_or("")),
            Component::ParentDir => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            _ => {}
        }
    }
    out.iter().collect()
}

/// `/assets/<rest>` (Starlette `StaticFiles(directory=web/dist/assets)`).
pub async fn asset(dist: &Path, rest: &str, method: &Method, headers: &HeaderMap) -> Response<Body> {
    if method != Method::GET && method != Method::HEAD {
        return plain(405, "Method Not Allowed");
    }
    let Ok(dir) = std::fs::canonicalize(dist.join("assets")) else { return plain(404, "Not Found") };
    let rel = normpath(&rest.split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/"));
    let full = dir.join(&rel);
    let full = std::fs::canonicalize(&full).unwrap_or(full);
    if !full.starts_with(&dir) || !full.is_file() {
        return plain(404, "Not Found");
    }
    let inm = headers.get("if-none-match").and_then(|v| v.to_str().ok());
    let mut r = file_response(&full, inm).await;
    // Vite puts a content hash in every file name under assets/: a changed file is a new URL, so the
    // browser and Cloudflare's edge may keep these for good (no revalidation round trip on each visit)
    if r.status() == StatusCode::OK || r.status() == StatusCode::NOT_MODIFIED {
        r.headers_mut().insert("cache-control", HeaderValue::from_static("public, max-age=31536000, immutable"));
    }
    r
}

/// The SPA catch-all `GET /{path:path}`.
pub async fn catch_all(dist: &Path, path: &str) -> Response<Body> {
    let path = path.strip_prefix('/').unwrap_or(path);
    let f = dist.join(path);
    if !path.is_empty() && f.is_file() {
        if let Ok(real) = std::fs::canonicalize(&f) {
            if real.ancestors().skip(1).any(|a| a == dist) {
                return file_response(&f, None).await;
            }
        }
    }
    // the page names the current asset files: always revalidate it, so a release is picked up at once
    let mut r = file_response(&dist.join("index.html"), None).await;
    r.headers_mut().insert("cache-control", HeaderValue::from_static("no-cache"));
    r
}
