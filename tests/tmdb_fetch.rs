//! Upstream fetch against a stubbed CDN.
//!
//! Every test runs against `wiremock`, so the suite needs no network, no
//! credentials and no TMDB availability. That matters beyond convenience: the
//! guards below are asserted against responses a real CDN would never send,
//! which is precisely why they cannot be tested against a real one.

use std::time::Duration;

use poster_service::tmdb::fetch::{self, FetchError, FetchLimits};
use poster_service::tmdb::{PosterPath, TmdbSize};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const JPEG: &[u8] = include_bytes!("fixtures/probe_37x53.jpg");
const PNG: &[u8] = include_bytes!("fixtures/probe_37x53.png");

fn limits() -> FetchLimits {
    FetchLimits {
        max_bytes: 4096,
        max_dimension: 8000,
        timeout: Duration::from_secs(2),
    }
}

/// Builds a PNG header declaring arbitrary dimensions, with no pixel data.
///
/// The shape of a decompression bomb as the guard sees it: a header claiming
/// an enormous decode target, in a file of 24 bytes.
fn png_header(width: u32, height: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend_from_slice(&13_u32.to_be_bytes());
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out
}

async fn serve(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/w780/kqjL17yufvn9OVLyXYpvtyrFfak.jpg"))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

fn url(server: &MockServer) -> String {
    PosterPath::parse("/kqjL17yufvn9OVLyXYpvtyrFfak.jpg")
        .expect("valid path")
        .cdn_url(&server.uri(), TmdbSize::W780)
}

#[tokio::test]
async fn fetches_artwork_and_reads_its_dimensions() {
    let server = serve(ResponseTemplate::new(200).set_body_bytes(JPEG)).await;
    let client = fetch::client(limits()).expect("client builds");

    let image = fetch::fetch_image(&client, &url(&server), limits())
        .await
        .expect("fetch succeeds");

    assert_eq!(image.bytes, JPEG);
    assert_eq!((image.dimensions.width, image.dimensions.height), (37, 53));
}

#[tokio::test]
async fn a_missing_source_is_distinguishable_from_an_outage() {
    // 404 must not present as a retryable upstream failure: the caller named
    // artwork that does not exist, and a retry cannot succeed.
    let server = serve(ResponseTemplate::new(404)).await;
    let client = fetch::client(limits()).expect("client builds");

    let error = fetch::fetch_image(&client, &url(&server), limits())
        .await
        .expect_err("404 must fail");

    assert!(matches!(error, FetchError::NotFound), "got {error:?}");
}

#[tokio::test]
async fn an_upstream_error_is_reported_with_its_status() {
    let server = serve(ResponseTemplate::new(503)).await;
    let client = fetch::client(limits()).expect("client builds");

    let error = fetch::fetch_image(&client, &url(&server), limits())
        .await
        .expect_err("503 must fail");

    assert!(
        matches!(error, FetchError::UnexpectedStatus { status } if status == 503),
        "got {error:?}"
    );
}

#[tokio::test]
async fn an_oversized_body_is_rejected() {
    let oversized = vec![0_u8; limits().max_bytes + 1];
    let server = serve(ResponseTemplate::new(200).set_body_bytes(oversized)).await;
    let client = fetch::client(limits()).expect("client builds");

    let error = fetch::fetch_image(&client, &url(&server), limits())
        .await
        .expect_err("oversized body must fail");

    assert!(
        matches!(error, FetchError::TooLarge { .. }),
        "got {error:?}"
    );
}

/// Serves one chunked HTTP response of `body_bytes` zero bytes, with no
/// `Content-Length` header, then closes.
///
/// Written against a raw socket because `wiremock` always declares a length:
/// hyper refuses to emit a body that contradicts `Content-Length`, so the
/// unbounded case cannot be expressed through it.
async fn serve_chunked_without_length(body_bytes: usize) -> String {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let addr = listener.local_addr().expect("has an address");

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accepts");

        // Drain the request line and headers; the content is irrelevant.
        let mut scratch = [0_u8; 1024];
        let _ = socket.read(&mut scratch).await;

        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await;

        // 4 KiB per chunk, so the cap is crossed partway through the stream
        // rather than on a single oversized write.
        let chunk = vec![0_u8; 4096];
        let mut sent = 0;
        while sent < body_bytes {
            if socket
                .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                .await
                .is_err()
            {
                // The client abandoned the transfer, which is the behaviour
                // under test.
                return;
            }
            if socket.write_all(&chunk).await.is_err() {
                return;
            }
            if socket.write_all(b"\r\n").await.is_err() {
                return;
            }
            sent += chunk.len();
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
    });

    format!("http://{addr}/w780/kqjL17yufvn9OVLyXYpvtyrFfak.jpg")
}

#[tokio::test]
async fn a_response_with_no_declared_length_is_still_capped() {
    // The case the streamed count exists for. Under chunked encoding there is
    // no Content-Length to consult, so a cap that trusted the header would
    // have nothing to check and would read until memory ran out.
    let url = serve_chunked_without_length(limits().max_bytes * 8).await;
    let client = fetch::client(limits()).expect("client builds");

    let error = fetch::fetch_image(&client, &url, limits())
        .await
        .expect_err("an unbounded chunked body must be capped");

    assert!(
        matches!(error, FetchError::TooLarge { .. }),
        "got {error:?}"
    );
}

#[tokio::test]
async fn a_declared_decompression_bomb_is_rejected_from_its_header() {
    // 24 bytes on the wire declaring a 60000x60000 decode target -- roughly
    // 14 GB of RGBA. The byte cap cannot catch this; only the header can.
    let bomb = png_header(60_000, 60_000);
    assert!(bomb.len() < 64, "the point is that the file is tiny");

    let server = serve(ResponseTemplate::new(200).set_body_bytes(bomb)).await;
    let client = fetch::client(limits()).expect("client builds");

    let error = fetch::fetch_image(&client, &url(&server), limits())
        .await
        .expect_err("a declared bomb must fail");

    assert!(
        matches!(
            error,
            FetchError::DimensionsExceeded {
                width: 60_000,
                height: 60_000,
                max: 8_000
            }
        ),
        "got {error:?}"
    );
}

#[tokio::test]
async fn dimensions_are_checked_per_side_not_by_area() {
    // A long thin image has a modest pixel count but still allocates a row
    // buffer proportional to its width.
    let server = serve(ResponseTemplate::new(200).set_body_bytes(png_header(50_000, 4))).await;
    let client = fetch::client(limits()).expect("client builds");

    let error = fetch::fetch_image(&client, &url(&server), limits())
        .await
        .expect_err("an overwide image must fail");

    assert!(
        matches!(error, FetchError::DimensionsExceeded { width: 50_000, .. }),
        "got {error:?}"
    );
}

#[tokio::test]
async fn artwork_at_the_dimension_limit_is_accepted() {
    // The bound is inclusive; an off-by-one here would reject valid artwork.
    let at_limit = png_header(8000, 8000);
    let server = serve(ResponseTemplate::new(200).set_body_bytes(at_limit)).await;
    let client = fetch::client(limits()).expect("client builds");

    let image = fetch::fetch_image(&client, &url(&server), limits())
        .await
        .expect("8000px artwork is within the limit");

    assert_eq!(image.dimensions.width, 8000);
}

#[tokio::test]
async fn a_non_image_response_is_rejected() {
    let server = serve(ResponseTemplate::new(200).set_body_string("<html>error page</html>")).await;
    let client = fetch::client(limits()).expect("client builds");

    let error = fetch::fetch_image(&client, &url(&server), limits())
        .await
        .expect_err("html must not be accepted as artwork");

    assert!(
        matches!(error, FetchError::UnrecognisedFormat),
        "got {error:?}"
    );
}

#[tokio::test]
async fn an_empty_body_is_rejected() {
    let server = serve(ResponseTemplate::new(200)).await;
    let client = fetch::client(limits()).expect("client builds");

    let error = fetch::fetch_image(&client, &url(&server), limits())
        .await
        .expect_err("an empty body has no dimensions");

    assert!(
        matches!(error, FetchError::UnrecognisedFormat),
        "got {error:?}"
    );
}

#[tokio::test]
async fn a_slow_response_times_out() {
    let slow = FetchLimits {
        timeout: Duration::from_millis(150),
        ..limits()
    };
    let server = serve(
        ResponseTemplate::new(200)
            .set_body_bytes(PNG)
            .set_delay(Duration::from_secs(5)),
    )
    .await;
    let client = fetch::client(slow).expect("client builds");

    let error = fetch::fetch_image(&client, &url(&server), slow)
        .await
        .expect_err("a slow response must time out");

    assert!(matches!(error, FetchError::Timeout), "got {error:?}");
}

#[tokio::test]
async fn redirects_are_not_followed() {
    // The path grammar guarantees the first request reaches the configured
    // host. Following a redirect would hand that guarantee back to whatever
    // the upstream chose to return, so this is the second half of the SSRF
    // control rather than a preference.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/w780/kqjL17yufvn9OVLyXYpvtyrFfak.jpg"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "http://169.254.169.254/latest/"),
        )
        .mount(&server)
        .await;

    let client = fetch::client(limits()).expect("client builds");
    let error = fetch::fetch_image(&client, &url(&server), limits())
        .await
        .expect_err("a redirect must not be followed");

    assert!(
        matches!(error, FetchError::UnexpectedStatus { status } if status == 302),
        "a redirect to link-local metadata must surface as a status, not a fetch: {error:?}"
    );
}
