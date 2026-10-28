use speleodb_api::{ApiOpError, HttpClient, HttpError, client::*, types::*};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

async fn server(status: u16, content_type: &str, body: &[u8]) -> (String, JoinHandle<Vec<u8>>) {
    server_with_headers(status, content_type, body, &[]).await
}

async fn server_with_headers(
    status: u16,
    content_type: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> (String, JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let extra = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    let response = [format!("HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nX-Witness: preserved\r\n{extra}Connection: close\r\n\r\n", body.len()).as_bytes(), body].concat();
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = socket.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0, "Client closed before completing its request");
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")
                                .map(|value| value.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            socket.write_all(&response).await.unwrap();
            request
        })
        .await
        .expect("Loopback request timed out")
    });
    (url, task)
}

fn request_body(request: &[u8]) -> &[u8] {
    let end = request
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .unwrap();
    &request[end + 4..]
}

async fn assert_stalled_response_times_out(client: HttpClient, elapsed: Duration) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client.with_base_url(format!("http://{}", listener.local_addr().unwrap()));
    let request = tokio::spawn(async move { client.health_status(None).await });

    // Complete real socket readiness before pausing time. The five-second
    // readiness guard is consumed here and cannot fire during virtual advances.
    let connection = tokio::time::timeout(Duration::from_secs(5), async {
        let (mut connection, _) = listener.accept().await.unwrap();
        let mut headers = Vec::new();
        let mut buffer = [0; 1024];
        while !headers.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = connection.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0, "Client closed before sending request headers");
            headers.extend_from_slice(&buffer[..count]);
        }
        connection
    })
    .await
    .expect("Client did not reach the stalled loopback server");

    tokio::time::pause();
    tokio::time::advance(elapsed).await;
    let error = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .expect("Stalled response exceeded the client deadline")
        .expect("Request task panicked")
        .expect_err("Stalled response unexpectedly succeeded");
    tokio::time::resume();
    drop(connection);

    match error {
        ApiOpError::Transport(HttpError::Network(error)) => assert!(error.is_timeout()),
        ApiOpError::Transport(HttpError::Middleware(error)) => assert!(error.is_timeout()),
        other => panic!("Expected a timeout transport error, received {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn default_client_enforces_its_configured_deadline_on_a_stalled_response() {
    assert_stalled_response_times_out(HttpClient::new(), Duration::from_secs(31)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn supplied_transport_retains_its_shorter_deadline() {
    let transport = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    assert_stalled_response_times_out(HttpClient::with_client(transport), Duration::from_secs(3))
        .await;
}

#[tokio::test]
async fn signed_download_redirects_preserve_location_and_keep_credentials_on_the_api() {
    for gis_source in [false, true] {
        // Storage responds promptly if contacted, so the old follow policy
        // fails the 302 assertion immediately instead of waiting for a timeout.
        let (storage_url, mut storage_request) = server(200, "application/zip", b"archive").await;
        let location = format!("{storage_url}/private.zip?signature=fixture");
        let (api_url, api_request) =
            server_with_headers(302, "text/plain", b"", &[("Location", &location)]).await;
        let client = HttpClient::new()
            .with_base_url(api_url)
            .with_api_key("fixture-bearer")
            .with_header("X-Session-Token", "fixture-session");
        let (status, actual_location) = if gis_source {
            let error = client
                .v2_gis_layers_source_retrieve("layer", None)
                .await
                .expect_err("Expected inspectable 302 handoff");
            let api = error.api().expect("Redirect must retain its HTTP envelope");
            (
                api.status,
                api.headers["location"].to_str().unwrap().to_owned(),
            )
        } else {
            let error = client
                .v2_user_exports_download_retrieve("export", None)
                .await
                .expect_err("Expected inspectable 302 handoff");
            let api = error.api().expect("Redirect must retain its HTTP envelope");
            (
                api.status,
                api.headers["location"].to_str().unwrap().to_owned(),
            )
        };
        assert_eq!(status, 302);
        assert_eq!(actual_location, location);
        let request = String::from_utf8(api_request.await.unwrap())
            .unwrap()
            .to_ascii_lowercase();
        assert!(request.contains("x-session-token: fixture-session\r\n"));
        assert!(request.contains("authorization: bearer fixture-bearer\r\n"));
        let contacted = tokio::time::timeout(Duration::from_millis(50), &mut storage_request).await;
        storage_request.abort();
        assert!(
            contacted.is_err(),
            "Signed storage received an API-client request"
        );
    }
}

#[tokio::test]
async fn supplied_transport_retains_its_explicit_follow_policy() {
    let (target_url, target_request) = server(200, "application/json", b"").await;
    let location = format!("{target_url}/health-target");
    let (api_url, api_request) =
        server_with_headers(302, "text/plain", b"", &[("Location", &location)]).await;
    let transport = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::limited(2))
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    HttpClient::with_client(transport)
        .with_base_url(api_url)
        .health_status(None)
        .await
        .unwrap();
    api_request.await.unwrap();
    assert!(
        String::from_utf8(target_request.await.unwrap())
            .unwrap()
            .starts_with("GET /health-target HTTP/1.1")
    );
}

fn assert_file_part(request: &[u8], field: &str, name: &str, mime: &str, bytes: &[u8]) {
    let headers_end = request
        .windows(4)
        .position(|value| value == b"\r\n\r\n")
        .unwrap();
    let headers = std::str::from_utf8(&request[..headers_end]).unwrap();
    let boundary = headers
        .lines()
        .find_map(|line| {
            line.split_once("boundary=")
                .map(|(_, boundary)| boundary.trim_matches('"'))
        })
        .unwrap();
    let marker = format!("--{boundary}\r\n");
    let ending = format!("\r\n--{boundary}");
    let body = request_body(request);
    for start in body
        .windows(marker.len())
        .enumerate()
        .filter_map(|(index, value)| (value == marker.as_bytes()).then_some(index + marker.len()))
    {
        let part = &body[start..];
        let header_end = part
            .windows(4)
            .position(|value| value == b"\r\n\r\n")
            .unwrap();
        let headers = std::str::from_utf8(&part[..header_end]).unwrap();
        if !headers.contains(&format!("name=\"{field}\"; filename=\"{name}\"")) {
            continue;
        }
        assert!(
            headers
                .to_ascii_lowercase()
                .contains(&format!("content-type: {mime}"))
        );
        let value = &part[header_end + 4..];
        let end = value
            .windows(ending.len())
            .position(|value| value == ending.as_bytes())
            .unwrap();
        assert_eq!(&value[..end], bytes);
        return;
    }
    panic!("Missing multipart file {field} with filename {name}");
}

#[tokio::test]
async fn resource_replacement_preserves_optional_binary_files_and_metadata() {
    let (url, received) = server(400, "application/json", br#"{"detail":"fixture"}"#).await;
    let request = V2ResourcesUpdateWithFormDataRequest::builder(
        V2ResourcesUpdateWithFormDataRequestResourceType::Photo,
        "Replacement".into(),
    )
    .file(
        MultipartFile::from(bytes::Bytes::from_static(b"\0\xffresource"))
            .with_file_name("resource.jpg")
            .with_content_type("application/octet-stream"),
    )
    .miniature(
        MultipartFile::from(bytes::Bytes::from_static(b"\xfe\0miniature"))
            .with_file_name("miniature.jpg")
            .with_content_type("image/jpeg"),
    )
    .build();
    let _ = HttpClient::new()
        .with_base_url(url)
        .v2_resources_update_with_form_data("resource", None, None::<&str>, request)
        .await;
    let request = received.await.unwrap();
    assert_file_part(
        &request,
        "file",
        "resource.jpg",
        "application/octet-stream",
        b"\0\xffresource",
    );
    assert_file_part(
        &request,
        "miniature",
        "miniature.jpg",
        "image/jpeg",
        b"\xfe\0miniature",
    );
}

#[tokio::test]
async fn resource_creation_preserves_optional_binary_miniature() {
    let (url, received) = server(400, "application/json", br#"{"detail":"fixture"}"#).await;
    let request = V2StationsResourcesCreateWithFormDataVariant2Request::builder(
        MultipartFile::from(bytes::Bytes::from_static(b"\0\xffphoto")).with_file_name("photo.jpg"),
        V2StationsResourcesCreateWithFormDataVariant2RequestResourceType::Photo,
        "Photo".into(),
    )
    .miniature(
        MultipartFile::from(bytes::Bytes::from_static(b"\xfe\0thumbnail"))
            .with_file_name("thumbnail.jpg")
            .with_content_type("image/jpeg"),
    )
    .build();
    let _ = HttpClient::new()
        .with_base_url(url)
        .v2_stations_resources_create_with_form_data_variant_2(
            "station",
            None,
            None::<&str>,
            request,
        )
        .await;
    let request = received.await.unwrap();
    assert_file_part(
        &request,
        "miniature",
        "thumbnail.jpg",
        "image/jpeg",
        b"\xfe\0thumbnail",
    );
}

#[tokio::test]
async fn log_creation_preserves_optional_binary_attachment() {
    let (url, received) = server(400, "application/json", br#"{"detail":"fixture"}"#).await;
    let request = V2StationsLogsCreateWithFormDataRequest::builder("Observation".into())
        .attachment(
            MultipartFile::from(bytes::Bytes::from_static(b"\xff\0attachment"))
                .with_file_name("notes.txt")
                .with_content_type("application/octet-stream"),
        )
        .build();
    let _ = HttpClient::new()
        .with_base_url(url)
        .v2_stations_logs_create_with_form_data("station", None, None::<&str>, request)
        .await;
    assert_file_part(
        &received.await.unwrap(),
        "attachment",
        "notes.txt",
        "application/octet-stream",
        b"\xff\0attachment",
    );
}

#[tokio::test]
async fn optional_binary_fields_distinguish_empty_clear_from_omission() {
    for clear in [false, true] {
        let (url, received) = server(400, "application/json", br#"{"detail":"fixture"}"#).await;
        let mut request = V2ResourcesUpdateWithFormDataRequest::new(
            V2ResourcesUpdateWithFormDataRequestResourceType::Photo,
            "Existing".into(),
        );
        if clear {
            request.file = Some(MultipartFile::from(bytes::Bytes::new()));
            request.miniature = Some(MultipartFile::from(bytes::Bytes::new()));
        }
        let _ = HttpClient::new()
            .with_base_url(url)
            .v2_resources_update_with_form_data("resource", None, None::<&str>, request)
            .await;
        let request = received.await.unwrap();
        let text = String::from_utf8_lossy(&request);
        for field in ["file", "miniature"] {
            assert_eq!(text.contains(&format!("name=\"{field}\"")), clear);
            if clear {
                assert!(text.contains(&format!("name=\"{field}\"\r\n\r\n\r\n")));
            }
        }
        assert!(!text.contains("filename="));
        assert!(
            !text
                .to_ascii_lowercase()
                .contains("content-type: application/json")
        );
    }
}

#[tokio::test]
async fn health_preserves_query_auth_and_empty_success() {
    let (url, received) = server(200, "application/json", b"").await;
    HttpClient::new()
        .with_base_url(url)
        .with_api_key("fixture-token")
        .health_status(Some(HealthStatusFormat::Json))
        .await
        .unwrap();
    let request = String::from_utf8(received.await.unwrap()).unwrap();
    assert!(request.starts_with("GET /api/health/?format=json HTTP/1.1\r\n"));
    assert!(
        request
            .to_lowercase()
            .contains("authorization: bearer fixture-token\r\n")
    );
}

#[tokio::test]
async fn declared_errors_and_unexpected_html_keep_status_headers_and_bytes() {
    for (status, media, body) in [
        (
            403,
            "application/json",
            br#"{"detail":"denied"}"#.as_slice(),
        ),
        (502, "text/html", b"<html>upstream failed</html>".as_slice()),
    ] {
        let (url, received) = server(status, media, body).await;
        let error = HttpClient::new()
            .with_base_url(url)
            .health_status(None)
            .await
            .unwrap_err();
        let api = error.api().unwrap();
        assert_eq!(api.status, status);
        assert_eq!(api.headers["x-witness"], "preserved");
        assert_eq!(api.raw_body, body);
        if status == 403 {
            assert!(matches!(
                api.typed,
                Some(HealthStatusApiError::Status403(_))
            ));
        }
        received.await.unwrap();
    }
}

#[tokio::test]
async fn json_is_encoded_once_and_distinguishes_absent_from_null() {
    let (url, received) = server(400, "application/json", br#"{"detail":"fixture"}"#).await;
    let body = serde_json::json!({"name":"Cavern", "description":"Survey", "country":"US", "type":"COMPASS", "latitude":null});
    let request: V2ProjectsCreateWithJsonRequest = serde_json::from_value(body.clone()).unwrap();
    let _ = HttpClient::new()
        .with_base_url(url)
        .v2_projects_create_with_json(None, Some("csrf-witness"), request)
        .await;
    let request = received.await.unwrap();
    let headers = String::from_utf8_lossy(&request).to_lowercase();
    assert!(headers.starts_with("post /api/v2/projects/ http/1.1"));
    assert!(headers.contains("content-type: application/json\r\n"));
    assert!(headers.contains("x-csrftoken: csrf-witness\r\n"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(request_body(&request)).unwrap(),
        body
    );
}

#[tokio::test]
async fn urlencoded_variant_uses_the_original_path_and_real_form_encoding() {
    let (url, received) = server(400, "application/json", br#"{"detail":"fixture"}"#).await;
    let body = serde_json::json!({"name":"A & B", "description":"Survey", "country":"US", "type":"COMPASS"});
    let request: V2ProjectsCreateWithUrlEncodedRequest = serde_json::from_value(body).unwrap();
    let _ = HttpClient::new()
        .with_base_url(url)
        .v2_projects_create_with_url_encoded(None, None::<&str>, request)
        .await;
    let request = received.await.unwrap();
    let headers = String::from_utf8_lossy(&request).to_lowercase();
    assert!(headers.starts_with("post /api/v2/projects/ http/1.1"));
    assert!(headers.contains("content-type: application/x-www-form-urlencoded\r\n"));
    let fields: std::collections::BTreeMap<_, _> =
        url::form_urlencoded::parse(request_body(&request))
            .into_owned()
            .collect();
    assert_eq!(fields["name"], "A & B");
    assert!(!fields.contains_key("latitude"));
}

#[tokio::test]
async fn urlencoded_explicit_clear_reaches_the_wire_as_an_empty_value() {
    let (url, received) = server(400, "application/json", br#"{"detail":"fixture"}"#).await;
    let request: V2ProjectsCreateWithUrlEncodedRequest = serde_json::from_value(serde_json::json!({
        "name":"Cavern", "description":"Survey", "country":"US", "type":"COMPASS", "latitude":"", "longitude":""
    })).unwrap();
    let _ = HttpClient::new()
        .with_base_url(url)
        .v2_projects_create_with_url_encoded(None, None::<&str>, request)
        .await;
    let request = received.await.unwrap();
    let fields: std::collections::BTreeMap<_, _> =
        url::form_urlencoded::parse(request_body(&request))
            .into_owned()
            .collect();
    assert_eq!(fields.get("latitude").map(String::as_str), Some(""));
    assert_eq!(fields.get("longitude").map(String::as_str), Some(""));
}

#[tokio::test]
async fn multipart_union_branch_is_callable_and_sends_scalar_fields() {
    let (url, received) = server(400, "application/json", br#"{"detail":"fixture"}"#).await;
    let request: V2StationsUpdateWithFormDataVariant2Request =
        serde_json::from_value(serde_json::json!({
            "name":"Station", "latitude":"10.5", "longitude":-20.5
        }))
        .unwrap();
    let _ = HttpClient::new()
        .with_base_url(url)
        .v2_stations_update_with_form_data_variant_2("station/id", None, None::<&str>, request)
        .await;
    let request = received.await.unwrap();
    let text = String::from_utf8_lossy(&request);
    assert!(text.starts_with("PUT /api/v2/stations/station%2Fid/ HTTP/1.1"));
    assert!(text.contains("name=\"latitude\"\r\n\r\n10.5"));
    assert!(text.contains("name=\"longitude\"\r\n\r\n-20.5"));
}

#[tokio::test]
async fn multipart_dynamic_experiment_fields_are_preserved() {
    let (url, received) = server(400, "application/json", br#"{"detail":"fixture"}"#).await;
    let request: V2StationsExperimentRecordsCreateWithFormDataRequest =
        serde_json::from_value(serde_json::json!({
            "00000000-0000-0000-0000-000000000001":"2026-10-28",
            "00000000-0000-0000-0000-000000000002":"text & witness"
        }))
        .unwrap();
    let _ = HttpClient::new()
        .with_base_url(url)
        .v2_stations_experiment_records_create_with_form_data(
            "experiment",
            "station",
            None,
            None::<&str>,
            request,
        )
        .await;
    let request = received.await.unwrap();
    let text = String::from_utf8_lossy(&request);
    assert!(text.contains("name=\"00000000-0000-0000-0000-000000000001\"\r\n\r\n2026-10-28"));
    assert!(text.contains("name=\"00000000-0000-0000-0000-000000000002\"\r\n\r\ntext & witness"));
}

#[tokio::test]
async fn multipart_upload_preserves_repeated_files_names_mime_and_binary() {
    let (url, received) = server(400, "application/json", br#"{"detail":"fixture"}"#).await;
    let request = ProjectUploadRequestRequest {
        message: "Upload witness".into(),
        artifact: vec![
            MultipartFile::from(bytes::Bytes::from_static(b"\0\xffsurvey"))
                .with_file_name("survey.dat")
                .with_content_type("application/octet-stream"),
            MultipartFile::from(bytes::Bytes::from_static(b"second")).with_file_name("second.dat"),
        ],
    };
    let _ = HttpClient::new()
        .with_base_url(url)
        .v2_projects_upload("COMPASS", "project/id", None, None::<&str>, request)
        .await;
    let request = received.await.unwrap();
    let text = String::from_utf8_lossy(&request);
    assert!(text.starts_with("PUT /api/v2/projects/project%2Fid/upload/COMPASS/ HTTP/1.1"));
    assert_eq!(text.matches("name=\"artifact\"").count(), 2);
    assert!(text.contains("filename=\"survey.dat\""));
    assert!(
        text.to_lowercase()
            .contains("content-type: application/octet-stream")
    );
    assert!(request.windows(8).any(|part| part == b"\0\xffsurvey"));
    assert!(text.contains("Upload witness"));
}

#[tokio::test]
async fn git_binary_response_and_path_encoding_are_lossless() {
    let body = b"\0\xffpacket";
    let (url, received) = server(200, "application/x-git-upload-pack-advertisement", body).await;
    let result = HttpClient::new()
        .with_base_url(url)
        .git_info_refs_builder("a/b ?", GitInfoRefsService::GitUploadPack)
        .send()
        .await
        .unwrap();
    assert_eq!(result.as_ref(), body);
    let request = String::from_utf8(received.await.unwrap()).unwrap();
    assert!(
        request.starts_with("GET /git/a%2Fb%20%3F.git/info/refs?service=git-upload-pack HTTP/1.1")
    );
}

#[tokio::test]
async fn malformed_json_and_undocumented_success_are_inspectable_errors() {
    for (status, body) in [(200, b"not json".as_slice()), (202, b"[]".as_slice())] {
        let (url, received) = server(status, "application/json", body).await;
        let error = HttpClient::new()
            .with_base_url(url)
            .v2_projects_list(None)
            .await
            .unwrap_err();
        let api = error.api().unwrap();
        assert_eq!(api.status, status);
        assert_eq!(api.raw_body, body);
        assert!(api.parse_error.is_some());
        received.await.unwrap();
    }
}

#[tokio::test]
async fn response_limit_applies_before_decoding() {
    let (url, received) = server(200, "application/json", b"oversized").await;
    let error = HttpClient::new()
        .with_base_url(url)
        .with_max_response_body_bytes(4)
        .health_status(None)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ApiOpError::Transport(HttpError::ResponseTooLarge { limit: 4 })
    ));
    received.await.unwrap();
}

#[tokio::test]
async fn connection_failures_remain_transport_errors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let transport = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let error = HttpClient::with_client(transport)
        .with_base_url(url)
        .health_status(None)
        .await
        .unwrap_err();
    assert!(matches!(error, ApiOpError::Transport(_)));
}
