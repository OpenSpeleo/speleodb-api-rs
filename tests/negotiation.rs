//! The download view negotiates JSON/HTML before its binary/redirect
//! response. A binary-only Accept fails before authentication or lookup.
use speleodb_api::{HttpClient, HttpError};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

async fn json_renderer(status: u16, body: &'static str) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&buffer[..count]);
            }
            let request = String::from_utf8(request).unwrap();
            let accept = request.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("accept").then(|| value.trim().to_owned())
            }).unwrap_or_else(|| "*/*".into());
            // Stage exposes JSONRenderer and BrowsableAPIRenderer. Live tests
            // exercise DRF itself, including format filtering and precedence.
            let can_render = accept.split(',').any(|media| matches!(
                media.split(';').next().unwrap().trim(),
                "application/json" | "application/*" | "text/html" | "text/*" | "*/*"
            ));
            let (status, body) = if can_render { (status, body) } else {
                (406, r#"{"detail":"Could not satisfy the request Accept header."}"#)
            };
            let location = if status == 302 { "Location: https://storage.example.invalid/project.zip\r\n" } else { "" };
            socket.write_all(format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{location}Connection: close\r\n\r\n{body}", body.len()
            ).as_bytes()).await.unwrap();
            accept
        }).await.expect("Renderer fixture timed out")
    });
    (base, task)
}

#[tokio::test]
async fn binary_only_accept_reproduces_server_406_before_download() {
    let (base, request) = json_renderer(302, "").await;
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!(
            "{base}/api/v2/projects/project/download/compass_zip/"
        ))
        .header("Accept", "application/octet-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 406);
    assert!(response.text().await.unwrap().contains("Could not satisfy"));
    assert_eq!(request.await.unwrap(), "application/octet-stream");
}

#[tokio::test]
async fn buffered_and_streaming_downloads_negotiate_json_before_the_handoff() {
    for streaming in [false, true] {
        let (base, request) = json_renderer(302, "").await;
        let client = HttpClient::new().with_base_url(base);
        let response = if streaming {
            match client
                .v2_projects_download_retrieve_by_format_binary_stream(
                    "compass_zip",
                    "project",
                    None,
                )
                .await
            {
                Err(error) => error,
                Ok(_) => panic!("Expected a preserved redirect"),
            }
        } else {
            client
                .v2_projects_download_retrieve_by_format("compass_zip", "project", None)
                .await
                .unwrap_err()
        };
        let response = response.api().unwrap();
        assert_eq!(
            response.status, 302,
            "Negotiation must reach the download view"
        );
        assert_eq!(
            response.headers["location"],
            "https://storage.example.invalid/project.zip"
        );
        let accept = request.await.unwrap();
        assert!(accept.contains("application/octet-stream"));
        assert!(accept.contains("application/json"));
    }
}

#[tokio::test]
async fn download_errors_keep_their_status_after_renderer_negotiation() {
    for status in [400, 403, 404, 422, 500] {
        let (base, request) = json_renderer(status, r#"{"error":"endpoint error"}"#).await;
        let error = HttpClient::new()
            .with_base_url(base)
            .v2_projects_download_retrieve_by_format("compass_zip", "project", None)
            .await
            .unwrap_err();
        assert_eq!(error.api().unwrap().status, status);
        assert_eq!(error.api().unwrap().body, r#"{"error":"endpoint error"}"#);
        request.await.unwrap();
    }
}

#[tokio::test]
async fn explicit_accept_overrides_are_case_insensitive() -> Result<(), HttpError> {
    let (base, request) = json_renderer(200, "").await;
    HttpClient::new()
        .with_base_url(base)
        .with_header("aCcEpT", "application/json; indent=4")
        .health_status(None)
        .await
        .unwrap();
    assert_eq!(request.await.unwrap(), "application/json; indent=4");
    Ok(())
}

#[tokio::test]
async fn explicit_download_accept_ranges_preserve_server_negotiation() {
    for (accept, expected) in [
        ("application/json", 302),
        ("text/html", 302),
        ("application/json; indent=4", 302),
        ("text/*", 302),
        ("application/*", 302),
        ("*/*", 302),
        ("application/octet-stream, application/json;q=0", 302),
        ("application/octet-stream", 406),
        ("application/xml", 406),
    ] {
        let (base, request) = json_renderer(302, "").await;
        let error = HttpClient::new()
            .with_base_url(base)
            .with_header("aCcEpT", accept)
            .v2_projects_download_retrieve_by_format("compass_zip", "project", None)
            .await
            .unwrap_err();
        assert_eq!(error.api().unwrap().status, expected, "Accept {accept}");
        assert_eq!(request.await.unwrap(), accept);
    }
}
