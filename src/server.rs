use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use bytes::Bytes;
use http::{header, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;

use crate::config::Config;
use crate::logger::Log;
use crate::random::{OsRandom, RandomSource};
use crate::store::{Store, StoreError};
use crate::wipe::{Wiper, WIPE_TOKEN_LENGTH};

pub const PLAN_ID_BYTES: usize = 12;
pub const MAX_ID_ATTEMPTS: usize = 5;
pub const PLAN_ID_LENGTH: usize = 16;

const LANDING_PAGE: &str = r#"<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Planista</title>
<style>
body{font:16px/1.5 system-ui,sans-serif;max-width:48rem;margin:4rem auto;padding:0 1rem;color:#202124}
code,pre{font-family:ui-monospace,monospace}pre{padding:1rem;background:#f4f4f5;overflow:auto}
</style>
<h1>Planista</h1>
<p>Post any file. Get a short public permalink.</p>
<pre>curl --fail-with-body -H 'Content-Type: video/mp4' --data-binary @demo.mp4 THIS_ORIGIN/</pre>
<p>Uploads are public, served with their supplied media type, and retained until an administrator wipes the server.</p>
</html>
"#;

pub struct Server {
    config: Config,
    store: Arc<Store>,
    wiper: Arc<Wiper>,
    random: Arc<dyn RandomSource>,
    logger: Arc<dyn Log>,
}

impl Server {
    pub fn new(config: Config, store: Arc<Store>, wiper: Arc<Wiper>, logger: Arc<dyn Log>) -> Self {
        Self {
            config,
            store,
            wiper,
            random: Arc::new(OsRandom),
            logger,
        }
    }

    pub fn set_random_for_test(&mut self, random: Arc<dyn RandomSource>) {
        self.random = random;
    }

    pub async fn handle_incoming(&self, req: Request<Incoming>) -> Response<Full<Bytes>> {
        self.handle(req).await
    }

    pub async fn handle<B>(&self, req: Request<B>) -> Response<Full<Bytes>>
    where
        B: http_body_util::BodyExt + Send,
        B::Data: Into<Bytes> + Send,
        B::Error: std::fmt::Display,
    {
        let path = req.uri().path().to_string();
        let method = req.method().clone();
        let range = req.headers().get(header::RANGE).cloned();

        if path.contains('%') || path.trim_start_matches('/').contains('/') {
            return not_found();
        }

        let segment = path.trim_start_matches('/');
        match segment {
            "" => self.handle_root(method, req).await,
            "healthz" => self.handle_health(method).await,
            s if s.len() == PLAN_ID_LENGTH && is_base64_url(s) => {
                self.handle_payload(method, s.to_string(), range).await
            }
            s if s.len() == WIPE_TOKEN_LENGTH && method == Method::POST => {
                self.handle_wipe(s.to_string()).await
            }
            _ => not_found(),
        }
    }

    async fn handle_root<B>(&self, method: Method, req: Request<B>) -> Response<Full<Bytes>>
    where
        B: http_body_util::BodyExt + Send,
        B::Data: Into<Bytes> + Send,
        B::Error: std::fmt::Display,
    {
        match method {
            Method::GET => {
                let body = LANDING_PAGE.replace("THIS_ORIGIN", &self.config.base_url);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(Full::new(Bytes::from(body)))
                    .unwrap()
            }
            Method::POST => self.handle_create(req).await,
            _ => method_not_allowed("GET, POST"),
        }
    }

    async fn handle_create<B>(&self, req: Request<B>) -> Response<Full<Bytes>>
    where
        B: http_body_util::BodyExt + Send,
        B::Data: Into<Bytes> + Send,
        B::Error: std::fmt::Display,
    {
        let content_type = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        if !is_valid_content_type(&content_type) {
            return error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Content-Type is required",
            );
        }

        let max = self.config.max_plan_bytes as usize;
        if let Some(cl) = req.headers().get(header::CONTENT_LENGTH) {
            if let Ok(n) = cl.to_str().unwrap_or("").parse::<usize>() {
                if n > max {
                    return error_response(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        payload_too_large_message(max),
                    );
                }
            }
        }

        let payload = match collect_body(req.into_body(), max).await {
            Ok(bytes) => bytes,
            Err(BodyError::TooLarge) => {
                return error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    payload_too_large_message(max),
                );
            }
            Err(BodyError::Other) => {
                return error_response(StatusCode::BAD_REQUEST, "could not read request body");
            }
        };

        if payload.is_empty() {
            return error_response(StatusCode::BAD_REQUEST, "payload must not be empty");
        }

        let mut last_err: Option<StoreError> = None;
        let mut id = String::new();
        for _ in 0..MAX_ID_ATTEMPTS {
            match random_id(self.random.as_ref()) {
                Ok(generated) => id = generated,
                Err(err) => {
                    self.logger.printf(format_args!("generate plan ID: {err}"));
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "could not create plan",
                    );
                }
            }
            match self
                .store
                .create_async(id.clone(), payload.clone(), content_type.clone())
                .await
            {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(StoreError::IdCollision) => {
                    last_err = Some(StoreError::IdCollision);
                    continue;
                }
                Err(err) => {
                    last_err = Some(err);
                    break;
                }
            }
        }

        match last_err {
            None => {
                let permalink = format!("{}/{}", self.config.base_url, id);
                Response::builder()
                    .status(StatusCode::CREATED)
                    .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                    .header(header::LOCATION, &permalink)
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(Full::new(Bytes::from(format!("{permalink}\n"))))
                    .unwrap()
            }
            Some(StoreError::AtCapacity) => {
                error_response(StatusCode::INSUFFICIENT_STORAGE, "plan limit reached")
            }
            Some(StoreError::IdCollision) => {
                self.logger.printf(format_args!(
                    "could not allocate a unique plan ID after {MAX_ID_ATTEMPTS} attempts"
                ));
                error_response(StatusCode::INTERNAL_SERVER_ERROR, "could not create plan")
            }
            Some(err) => {
                self.logger.printf(format_args!("store plan: {err}"));
                error_response(StatusCode::INTERNAL_SERVER_ERROR, "could not create plan")
            }
        }
    }

    async fn handle_payload(
        &self,
        method: Method,
        id: String,
        range_header: Option<http::HeaderValue>,
    ) -> Response<Full<Bytes>> {
        if method != Method::GET && method != Method::HEAD {
            return method_not_allowed("GET, HEAD");
        }
        match self.store.get_async(id).await {
            Ok(payload) => {
                let bytes = Bytes::from(payload.bytes);
                let full_len = bytes.len();
                let requested_range = range_header.as_ref().and_then(|value| value.to_str().ok());
                let range = match parse_byte_range(requested_range, full_len) {
                    Ok(range) => range,
                    Err(()) => return range_not_satisfiable(full_len),
                };
                let (status, response_bytes, content_range) = match range {
                    Some((start, end)) => (
                        StatusCode::PARTIAL_CONTENT,
                        bytes.slice(start..end),
                        Some(format!("bytes {start}-{}/{full_len}", end - 1)),
                    ),
                    None => (StatusCode::OK, bytes, None),
                };
                let len = response_bytes.len();
                let body = if method == Method::HEAD {
                    Full::new(Bytes::new())
                } else {
                    Full::new(response_bytes)
                };
                let mut response = Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, payload.content_type)
                    .header(header::ACCEPT_RANGES, "bytes")
                    .header(header::CONTENT_LENGTH, len.to_string())
                    .header(header::CACHE_CONTROL, "no-store")
                    .header(header::CONTENT_SECURITY_POLICY, "frame-ancestors 'none'")
                    .header(header::REFERRER_POLICY, "no-referrer")
                    .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
                if let Some(content_range) = content_range {
                    response = response.header(header::CONTENT_RANGE, content_range);
                }
                response.body(body).unwrap()
            }
            Err(StoreError::NotFound) => not_found(),
            Err(err) => {
                self.logger.printf(format_args!("get plan: {err}"));
                error_response(StatusCode::INTERNAL_SERVER_ERROR, "could not retrieve plan")
            }
        }
    }

    async fn handle_health(&self, method: Method) -> Response<Full<Bytes>> {
        if method != Method::GET {
            return method_not_allowed("GET");
        }
        match self.store.health_async().await {
            Ok(()) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-store")
                .body(Full::new(Bytes::from_static(b"ok\n")))
                .unwrap(),
            Err(err) => {
                self.logger.printf(format_args!("health check: {err}"));
                error_response(StatusCode::SERVICE_UNAVAILABLE, "unhealthy")
            }
        }
    }

    async fn handle_wipe(&self, token: String) -> Response<Full<Bytes>> {
        if !self.wiper.matches(&token) {
            return not_found();
        }
        match self.store.wipe_async().await {
            Ok(()) => Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Full::new(Bytes::new()))
                .unwrap(),
            Err(err) => {
                self.logger.printf(format_args!("wipe plans: {err}"));
                error_response(StatusCode::INTERNAL_SERVER_ERROR, "could not wipe plans")
            }
        }
    }
}

fn random_id(source: &dyn RandomSource) -> Result<String, String> {
    let mut raw = vec![0u8; PLAN_ID_BYTES];
    source.fill(&mut raw).map_err(|e| format!("{e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(raw))
}

fn is_base64_url(value: &str) -> bool {
    value
        .bytes()
        .all(|b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_'))
}

fn is_valid_content_type(value: &str) -> bool {
    let media_type = value.split(';').next().unwrap_or("").trim();
    let Some((type_, subtype)) = media_type.split_once('/') else {
        return false;
    };
    !subtype.contains('/') && is_mime_token(type_) && is_mime_token(subtype)
}

fn is_mime_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn payload_too_large_message(max: usize) -> String {
    let mib = 1 << 20;
    let limit = if max.is_multiple_of(mib) {
        format!("{} MiB", max / mib)
    } else {
        format!("{max} bytes")
    };
    format!(
        "payload exceeds the {limit} limit; compress the file (for video, try ffmpeg) or increase PLANISTA_MAX_PLAN_BYTES"
    )
}

fn parse_byte_range(value: Option<&str>, len: usize) -> Result<Option<(usize, usize)>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    let range = value.strip_prefix("bytes=").ok_or(())?;
    if range.contains(',') {
        return Err(());
    }
    let (start, end) = range.split_once('-').ok_or(())?;
    if start.is_empty() {
        let suffix = end.parse::<usize>().map_err(|_| ())?;
        if suffix == 0 || len == 0 {
            return Err(());
        }
        return Ok(Some((len.saturating_sub(suffix), len)));
    }

    let start = start.parse::<usize>().map_err(|_| ())?;
    if start >= len {
        return Err(());
    }
    let end = if end.is_empty() {
        len
    } else {
        end.parse::<usize>()
            .map_err(|_| ())?
            .saturating_add(1)
            .min(len)
    };
    if end <= start {
        return Err(());
    }
    Ok(Some((start, end)))
}

enum BodyError {
    TooLarge,
    Other,
}

async fn collect_body<B>(body: B, max: usize) -> Result<Vec<u8>, BodyError>
where
    B: http_body_util::BodyExt,
    B::Data: Into<Bytes>,
    B::Error: std::fmt::Display,
{
    let mut out = Vec::new();
    let mut body = std::pin::pin!(body);
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| BodyError::Other)?;
        if let Ok(data) = frame.into_data() {
            let data: Bytes = data.into();
            if out.len().saturating_add(data.len()) > max {
                return Err(BodyError::TooLarge);
            }
            out.extend_from_slice(&data);
        }
    }
    Ok(out)
}

fn error_response(status: StatusCode, msg: impl AsRef<str>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Full::new(Bytes::from(format!("{}\n", msg.as_ref()))))
        .unwrap()
}

fn range_not_satisfiable(len: usize) -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(header::CONTENT_RANGE, format!("bytes */{len}"))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Full::new(Bytes::from_static(
            b"requested range is not satisfiable\n",
        )))
        .unwrap()
}

fn not_found() -> Response<Full<Bytes>> {
    error_response(StatusCode::NOT_FOUND, "not found")
}

fn method_not_allowed(allow: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .header(header::ALLOW, allow)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Full::new(Bytes::from_static(b"method not allowed\n")))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logger::CaptureLogger;
    use crate::random::SeqRandom;
    use crate::wipe::WIPE_TOKEN_LENGTH;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    fn temp_db() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "planista-srv-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path.join("planista.db")
    }

    async fn new_test_server(max_bytes: i64, max_plans: usize) -> (Server, Arc<Store>, Arc<Wiper>) {
        let store = Arc::new(Store::open(temp_db(), max_plans).unwrap());
        let logger: Arc<dyn Log> = Arc::new(CaptureLogger::new());
        let wiper = Arc::new(
            Wiper::new(
                "https://plans.example.com".into(),
                Duration::from_secs(120),
                logger.clone(),
            )
            .unwrap(),
        );
        let cfg = Config {
            base_url: "https://plans.example.com".into(),
            listen_addr: ":8080".into(),
            db_path: "/tmp/x.db".into(),
            max_plan_bytes: max_bytes,
            max_plans,
            wipe_interval: Duration::from_secs(120),
        };
        (
            Server::new(cfg, Arc::clone(&store), Arc::clone(&wiper), logger),
            store,
            wiper,
        )
    }

    async fn request(
        server: &Server,
        method: Method,
        path: &str,
        body: &str,
        content_type: Option<&str>,
    ) -> Response<Full<Bytes>> {
        request_bytes(server, method, path, body.as_bytes(), content_type, None).await
    }

    async fn request_bytes(
        server: &Server,
        method: Method,
        path: &str,
        body: &[u8],
        content_type: Option<&str>,
        range: Option<&str>,
    ) -> Response<Full<Bytes>> {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(ct) = content_type {
            builder = builder.header(header::CONTENT_TYPE, ct);
        }
        if let Some(range) = range {
            builder = builder.header(header::RANGE, range);
        }
        let req = builder
            .body(Full::new(Bytes::copy_from_slice(body)))
            .unwrap();
        server.handle(req).await
    }

    async fn upload(server: &Server, body: &str, content_type: &str) -> Response<Full<Bytes>> {
        request(server, Method::POST, "/", body, Some(content_type)).await
    }

    async fn body_string(resp: Response<Full<Bytes>>) -> (StatusCode, http::HeaderMap, String) {
        let (status, headers, bytes) = body_bytes(resp).await;
        (
            status,
            headers,
            String::from_utf8_lossy(&bytes).into_owned(),
        )
    }

    async fn body_bytes(resp: Response<Full<Bytes>>) -> (StatusCode, http::HeaderMap, Bytes) {
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, headers, bytes)
    }

    #[tokio::test]
    async fn plan_lifecycle() {
        let (server, _, _) = new_test_server(1024, 10).await;
        let html = "<!doctype html><script>document.body.textContent='active'</script>";
        let (status, headers, body) =
            body_string(upload(&server, html, "text/html; charset=utf-8").await).await;
        assert_eq!(status, StatusCode::CREATED);
        let permalink = body.trim();
        assert_eq!(
            headers.get(header::LOCATION).unwrap().to_str().unwrap(),
            permalink
        );
        let id = permalink
            .strip_prefix("https://plans.example.com/")
            .unwrap();
        assert_eq!(id.len(), PLAN_ID_LENGTH);
        assert!(is_base64_url(id));

        let (status, headers, got) =
            body_string(request(&server, Method::GET, &format!("/{id}"), "", None).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(got, html);
        assert_eq!(
            headers.get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            headers.get(header::CONTENT_SECURITY_POLICY).unwrap(),
            "frame-ancestors 'none'"
        );

        let (status, headers, got) =
            body_string(request(&server, Method::HEAD, &format!("/{id}"), "", None).await).await;
        assert_eq!(status, StatusCode::OK);
        assert!(got.is_empty());
        assert_eq!(
            headers
                .get(header::CONTENT_LENGTH)
                .unwrap()
                .to_str()
                .unwrap(),
            html.len().to_string()
        );

        let (_, _, body2) = body_string(upload(&server, html, "text/html").await).await;
        assert_ne!(body, body2);
    }

    #[tokio::test]
    async fn upload_validation_and_methods() {
        let (server, _, _) = new_test_server(4, 1).await;
        let cases = [
            (
                Method::POST,
                "/",
                "html",
                Some("not-a-media-type"),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                Method::POST,
                "/",
                "html",
                None,
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                Method::POST,
                "/",
                "",
                Some("text/html"),
                StatusCode::BAD_REQUEST,
            ),
            (
                Method::POST,
                "/",
                "12345",
                Some("text/html"),
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
            (
                Method::DELETE,
                "/",
                "",
                None,
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (
                Method::POST,
                "/healthz",
                "",
                None,
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (Method::GET, "/a/b", "", None, StatusCode::NOT_FOUND),
            (
                Method::GET,
                "/abcdefghijklmnop",
                "",
                None,
                StatusCode::NOT_FOUND,
            ),
        ];
        for (method, path, body, ct, want) in cases {
            let (status, _, _) =
                body_string(request(&server, method.clone(), path, body, ct).await).await;
            assert_eq!(status, want, "{method} {path}");
        }

        let (status, _, _) = body_string(upload(&server, "1234", "text/html").await).await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, _, _) = body_string(upload(&server, "1234", "text/html").await).await;
        assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE);
    }

    #[tokio::test]
    async fn serves_binary_payloads_and_byte_ranges() {
        let (server, _, _) = new_test_server(1024, 10).await;
        let video: Vec<u8> = (0..10).collect();
        let (status, headers, body) = body_string(
            request_bytes(&server, Method::POST, "/", &video, Some("video/mp4"), None).await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let id = body
            .trim()
            .strip_prefix("https://plans.example.com/")
            .unwrap();
        assert_eq!(headers.get(header::LOCATION).unwrap(), body.trim());

        let (status, headers, got) = body_bytes(
            request_bytes(&server, Method::GET, &format!("/{id}"), &[], None, None).await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "video/mp4");
        assert_eq!(headers.get(header::ACCEPT_RANGES).unwrap(), "bytes");
        assert_eq!(got.as_ref(), video);

        let (status, headers, got) = body_bytes(
            request_bytes(
                &server,
                Method::GET,
                &format!("/{id}"),
                &[],
                None,
                Some("bytes=2-5"),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers.get(header::CONTENT_RANGE).unwrap(), "bytes 2-5/10");
        assert_eq!(headers.get(header::CONTENT_LENGTH).unwrap(), "4");
        assert_eq!(got.as_ref(), &[2, 3, 4, 5]);

        let (status, headers, got) = body_bytes(
            request_bytes(
                &server,
                Method::HEAD,
                &format!("/{id}"),
                &[],
                None,
                Some("bytes=-3"),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers.get(header::CONTENT_RANGE).unwrap(), "bytes 7-9/10");
        assert_eq!(headers.get(header::CONTENT_LENGTH).unwrap(), "3");
        assert!(got.is_empty());

        let (status, headers, _) = body_bytes(
            request_bytes(
                &server,
                Method::GET,
                &format!("/{id}"),
                &[],
                None,
                Some("bytes=99-"),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(headers.get(header::CONTENT_RANGE).unwrap(), "bytes */10");
    }

    #[tokio::test]
    async fn too_large_error_suggests_compression() {
        let (server, _, _) = new_test_server(4, 10).await;
        let (status, _, body) = body_string(upload(&server, "12345", "video/mp4").await).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(body.contains("4 bytes limit"));
        assert!(body.contains("ffmpeg"));
        assert!(body.contains("PLANISTA_MAX_PLAN_BYTES"));
    }

    #[tokio::test]
    async fn wipe_endpoint_and_token_invalidation() {
        let (server, store, wiper) = new_test_server(1024, 10).await;
        let (_, _, body) = body_string(upload(&server, "<p>erase me</p>", "text/html").await).await;
        let id = body
            .trim()
            .strip_prefix("https://plans.example.com/")
            .unwrap()
            .to_string();
        let old_token = wiper.token_for_test();

        let (status, _, _) = body_string(
            request(
                &server,
                Method::POST,
                &format!("/{}", "x".repeat(WIPE_TOKEN_LENGTH)),
                "",
                None,
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        wiper.rotate().unwrap();
        let (status, _, _) =
            body_string(request(&server, Method::POST, &format!("/{old_token}"), "", None).await)
                .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let token = wiper.token_for_test();
        let (status, _, _) =
            body_string(request(&server, Method::GET, &format!("/{token}"), "", None).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _, _) =
            body_string(request(&server, Method::POST, &format!("/{token}"), "", None).await).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(store.get(&id), Err(StoreError::NotFound));
    }

    #[tokio::test]
    async fn id_collision_retries() {
        let (mut server, _, _) = new_test_server(1024, 10).await;
        let mut bytes = vec![0u8; PLAN_ID_BYTES];
        bytes.extend(vec![0u8; PLAN_ID_BYTES]);
        bytes.extend(vec![1u8; PLAN_ID_BYTES]);
        server.set_random_for_test(Arc::new(SeqRandom::new(bytes)));

        let (_, _, first) = body_string(upload(&server, "first", "text/html").await).await;
        let (_, _, second) = body_string(upload(&server, "second", "text/html").await).await;
        assert_ne!(first, second);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_uploads_respect_capacity() {
        const CAPACITY: usize = 10;
        const REQUESTS: usize = 30;
        let (server, _, _) = new_test_server(1024, CAPACITY).await;
        let server = Arc::new(server);
        let created = Arc::new(AtomicU32::new(0));
        let full = Arc::new(AtomicU32::new(0));
        let unexpected = Arc::new(AtomicU32::new(0));
        let mut handles = Vec::new();
        for i in 0..REQUESTS {
            let server = Arc::clone(&server);
            let created = Arc::clone(&created);
            let full = Arc::clone(&full);
            let unexpected = Arc::clone(&unexpected);
            handles.push(tokio::spawn(async move {
                let body = format!("<p>{i}</p>");
                let (status, _, _) = body_string(upload(&server, &body, "text/html").await).await;
                match status {
                    StatusCode::CREATED => {
                        created.fetch_add(1, Ordering::SeqCst);
                    }
                    StatusCode::INSUFFICIENT_STORAGE => {
                        full.fetch_add(1, Ordering::SeqCst);
                    }
                    _ => {
                        unexpected.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(created.load(Ordering::SeqCst), CAPACITY as u32);
        assert_eq!(full.load(Ordering::SeqCst), (REQUESTS - CAPACITY) as u32);
        assert_eq!(unexpected.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn health_and_landing_page() {
        let (server, _, _) = new_test_server(1024, 10).await;
        let (status, _, body) =
            body_string(request(&server, Method::GET, "/healthz", "", None).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok\n");
        let (status, _, body) =
            body_string(request(&server, Method::GET, "/", "", None).await).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Post any file"));
    }
}
