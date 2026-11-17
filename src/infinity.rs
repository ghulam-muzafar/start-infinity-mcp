use std::{sync::Arc, time::Duration};

use reqwest::{Client, StatusCode, Url, header};
use serde_json::Value;
use tokio::{sync::Mutex, time::Instant};

use crate::models::Page;

const BASE_URL: &str = "https://app.startinfinity.com/api/v2/";
pub const API_VERSION: &str = "2026-04-20.morava";
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const REQUEST_SPACING: Duration = Duration::from_millis(350);

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    InvalidInput(&'static str),
    #[error("Could not configure the Infinity HTTP client")]
    Configuration,
    #[error("Infinity request failed or timed out; check network access and try again")]
    Network,
    #[error("Infinity returned HTTP {status}: {hint}")]
    Http { status: u16, hint: &'static str },
    #[error("Infinity returned an invalid JSON response")]
    InvalidJson,
    #[error("Infinity response exceeded 2 MiB; request a smaller page")]
    ResponseTooLarge,
}

#[derive(Clone)]
pub struct InfinityClient {
    http: Client,
    base: Url,
    next_request: Arc<Mutex<Instant>>,
}

impl InfinityClient {
    pub fn new(token: &str) -> Result<Self, ApiError> {
        if token.trim().is_empty() {
            return Err(ApiError::InvalidInput(
                "INFINITY_API_TOKEN must not be empty",
            ));
        }
        let mut auth = header::HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            ApiError::InvalidInput("INFINITY_API_TOKEN is not a valid header value")
        })?;
        auth.set_sensitive(true);
        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, auth);
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            "X-API-Version",
            header::HeaderValue::from_static(API_VERSION),
        );
        let http = Client::builder()
            .default_headers(headers)
            .user_agent(concat!("start-infinity-mcp/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| ApiError::Configuration)?;
        Ok(Self {
            http,
            base: Url::parse(BASE_URL).map_err(|_| ApiError::Configuration)?,
            next_request: Arc::new(Mutex::new(Instant::now())),
        })
    }

    /// Only fixed GET routes are exposed by tools. IDs cannot inject path/query syntax.
    pub async fn get(
        &self,
        segments: &[&str],
        page: Option<&Page>,
        extra_query: &[(&str, &str)],
    ) -> Result<Value, ApiError> {
        for segment in segments {
            validate_id(segment)?;
        }
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| ApiError::Configuration)?
            .pop_if_empty()
            .extend(segments);
        if let Some(page) = page {
            let limit = page.limit.unwrap_or(25);
            if !(1..=100).contains(&limit) {
                return Err(ApiError::InvalidInput("limit must be between 1 and 100"));
            }
            if page.after.is_some() && page.before.is_some() {
                return Err(ApiError::InvalidInput(
                    "Use either after or before, not both",
                ));
            }
            let mut query = url.query_pairs_mut();
            query.append_pair("limit", &limit.to_string());
            for (key, value) in [("after", &page.after), ("before", &page.before)] {
                if let Some(value) = value {
                    if value.is_empty() || value.len() > 4096 {
                        return Err(ApiError::InvalidInput(
                            "Pagination cursors must contain 1–4096 bytes",
                        ));
                    }
                    query.append_pair(key, value);
                }
            }
        }
        if !extra_query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(extra_query.iter().copied());
        }

        for attempt in 0..3 {
            // Shared across cloned clients. Keep a margin below 180 requests/minute.
            {
                let mut next = self.next_request.lock().await;
                tokio::time::sleep_until(*next).await;
                *next = Instant::now() + REQUEST_SPACING;
            }
            let mut response = self
                .http
                .get(url.clone())
                .send()
                .await
                .map_err(|_| ApiError::Network)?;
            let status = response.status();
            if !status.is_success() {
                let retryable = matches!(status.as_u16(), 429 | 502 | 503 | 504);
                if retryable && attempt < 2 {
                    // Never retry earlier than Retry-After. Long or date-form delays
                    // are handed back to the caller rather than blocking the tool.
                    let delay = match response.headers().get(header::RETRY_AFTER) {
                        Some(value) => value
                            .to_str()
                            .ok()
                            .and_then(|v| v.parse::<u64>().ok())
                            .filter(|seconds| *seconds <= 5),
                        None => Some(1 << attempt),
                    };
                    if let Some(seconds) = delay {
                        tracing::warn!(status = status.as_u16(), "Retrying Infinity GET");
                        drop(response);
                        tokio::time::sleep(Duration::from_secs(seconds)).await;
                        continue;
                    }
                }
                return Err(http_error(status));
            }
            if response
                .content_length()
                .is_some_and(|size| size > MAX_BODY_BYTES as u64)
            {
                return Err(ApiError::ResponseTooLarge);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| ApiError::Network)? {
                if body.len() + chunk.len() > MAX_BODY_BYTES {
                    return Err(ApiError::ResponseTooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            return serde_json::from_slice(&body).map_err(|_| ApiError::InvalidJson);
        }
        unreachable!("The last attempt always returns")
    }
}

pub fn validate_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(ApiError::InvalidInput(
            "IDs must contain 1–128 ASCII letters, digits, hyphens or underscores",
        ));
    }
    Ok(())
}

fn http_error(status: StatusCode) -> ApiError {
    let hint = match status.as_u16() {
        401 => "check INFINITY_API_TOKEN",
        402 => "workspace subscription required; check workspace billing and API access",
        403 => "the token's user does not have permission",
        404 => "resource not found or not accessible to the token's user",
        429 => "rate limit reached; wait before trying again",
        500..=599 => "upstream service error; try again later",
        _ => "request rejected; check IDs and parameters",
    };
    // Do not forward upstream bodies, URLs or credentials into logs/tool errors.
    ApiError::Http {
        status: status.as_u16(),
        hint,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    pub fn response(status: u16, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    pub async fn mock(responses: Vec<String>) -> (InfinityClient, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = InfinityClient::new("test-only-token").unwrap();
        client.base = Url::parse(&format!(
            "http://{}/api/v2/",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .expect("Expected an HTTP request")
                        .unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0_u8; 2048];
                    let n = stream.read(&mut buffer).await.unwrap();
                    assert!(n > 0, "Client closed before sending headers");
                    request.extend_from_slice(&buffer[..n]);
                    assert!(request.len() < 16 * 1024);
                    if request.windows(4).any(|s| s == b"\r\n\r\n") {
                        break;
                    }
                }
                requests.push(String::from_utf8(request).unwrap());
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            }
            requests
        });
        (client, handle)
    }

    #[tokio::test]
    async fn preserves_payload_and_encodes_cursors_with_auth_and_version() {
        let body =
            r#"{"data":[{"id":"one","custom":{"unknown":true}}],"has_more":true,"after":"next"}"#;
        let (client, server) = mock(vec![response(200, "", body)]).await;
        let page = Page {
            limit: Some(100),
            after: Some("a+b/c==&extra=bad".into()),
            before: None,
        };
        let actual = client
            .get(&["workspaces", "669", "boards"], Some(&page), &[])
            .await
            .unwrap();
        assert_eq!(actual, serde_json::from_str::<Value>(body).unwrap());
        let requests = server.await.unwrap();
        let request = &requests[0];
        assert!(request.starts_with("GET /api/v2/workspaces/669/boards?"));
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("authorization: bearer test-only-token\r\n"));
        assert!(lower.contains(&format!("x-api-version: {API_VERSION}\r\n")));
        let target = request.split_whitespace().nth(1).unwrap();
        let url = Url::parse(&format!("http://localhost{target}")).unwrap();
        let query: Vec<_> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(
            query,
            vec![
                ("limit".into(), "100".into()),
                ("after".into(), "a+b/c==&extra=bad".into())
            ]
        );
    }

    #[tokio::test]
    async fn rejects_invalid_inputs_before_network_access() {
        let client = InfinityClient::new("test-only-token").unwrap();
        for id in ["", "..", "a/b", "a?b", "a%2Fb", "\n", "名字"] {
            assert!(matches!(
                client.get(&["workspaces", id], None, &[]).await,
                Err(ApiError::InvalidInput(_))
            ));
        }
        for page in [
            Page {
                limit: Some(0),
                ..Page::default()
            },
            Page {
                limit: Some(101),
                ..Page::default()
            },
            Page {
                after: Some("a".into()),
                before: Some("b".into()),
                ..Page::default()
            },
            Page {
                after: Some("".into()),
                ..Page::default()
            },
            Page {
                before: Some("a".repeat(4097)),
                ..Page::default()
            },
        ] {
            assert!(matches!(
                client.get(&["workspaces"], Some(&page), &[]).await,
                Err(ApiError::InvalidInput(_))
            ));
        }
        assert!(InfinityClient::new("  ").is_err());
        assert!(InfinityClient::new("token\r\ninjected: value").is_err());
    }

    #[tokio::test]
    async fn permission_errors_do_not_forward_sensitive_upstream_bodies() {
        for status in [401, 403, 404] {
            let (client, server) =
                mock(vec![response(status, "", "sensitive upstream body")]).await;
            let error = client.get(&["workspaces"], None, &[]).await.unwrap_err();
            assert!(error.to_string().contains(&status.to_string()));
            assert!(!error.to_string().contains("sensitive"));
            assert_eq!(server.await.unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn retries_transient_errors_but_stops_after_three_attempts() {
        let (client, server) = mock(vec![response(503, "Retry-After: 0\r\n", ""); 3]).await;
        assert!(matches!(
            client.get(&["workspaces"], None, &[]).await,
            Err(ApiError::Http { status: 503, .. })
        ));
        assert_eq!(server.await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn rate_limit_retry_can_recover() {
        let (client, server) = mock(vec![
            response(429, "Retry-After: 0\r\n", ""),
            response(200, "", "{\"data\":[]}"),
        ])
        .await;
        assert_eq!(
            client.get(&["workspaces"], None, &[]).await.unwrap(),
            serde_json::json!({"data": []})
        );
        assert_eq!(server.await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn long_or_date_retry_after_is_returned_without_retrying() {
        for retry in ["120", "Wed, 11 Nov 2026 00:00:00 GMT"] {
            let (client, server) = mock(vec![response(
                429,
                &format!("Retry-After: {retry}\r\n"),
                "",
            )])
            .await;
            assert!(matches!(
                client.get(&["workspaces"], None, &[]).await,
                Err(ApiError::Http { status: 429, .. })
            ));
            assert_eq!(server.await.unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn does_not_follow_redirects() {
        let (client, server) = mock(vec![response(
            302,
            "Location: https://example.invalid/\r\n",
            "",
        )])
        .await;
        assert!(matches!(
            client.get(&["workspaces"], None, &[]).await,
            Err(ApiError::Http { status: 302, .. })
        ));
        assert_eq!(server.await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn rejects_non_json_and_oversized_responses() {
        let (client, server) = mock(vec![response(200, "", "<html>not JSON</html>")]).await;
        assert!(matches!(
            client.get(&["workspaces"], None, &[]).await,
            Err(ApiError::InvalidJson)
        ));
        server.await.unwrap();
        // Advertised oversized length is rejected before reading the body.
        let (client, server) = mock(vec![format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            MAX_BODY_BYTES + 1
        )])
        .await;
        assert!(matches!(
            client.get(&["workspaces"], None, &[]).await,
            Err(ApiError::ResponseTooLarge)
        ));
        server.await.unwrap();
    }
}
