//! This module contains the service that is being served with Hyper (our HTTP server library). The
//! functions in here are responsible for taking requests from the GitHub API and producing
//! responses.

use crate::config::{self, Config};

use http_body_util::{combinators::BoxBody, BodyExt, Full, Empty};
use hyper::body::{Body, Bytes};
use hyper::header::{HeaderMap, HeaderValue};
use hyper::{Request, Response, Method, StatusCode};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::num::ParseIntError;

use tokio::process::Command;
use tokio::io::AsyncWriteExt;
use std::io;
use std::process::{ExitStatus, Stdio};

/// Alias for hasher implementing HMAC-SHA256.
type HmacSha256 = Hmac<Sha256>;

/// Dispatches HTTP requests to different handlers, returning their result.
pub async fn router(
    req: Request<hyper::body::Incoming>,
    config: &Config,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    match (req.method(), req.uri().path()) {
        (&Method::POST, "/") => handle_webhook_post(req, config).await,
        _ => Ok(empty_res(StatusCode::NOT_FOUND)),
    }
}

async fn handle_webhook_post(
    req: Request<hyper::body::Incoming>,
    config: &Config,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let (head, body) = req.into_parts();

    // Extract the event type early on. This allows us to exit before doing expensive signature
    // checking, if the header is missing or invalid ASCII.
    let event = match head.headers.get("X-GitHub-event").map(HeaderValue::to_str) {
        Some(Ok(event)) => event,
        Some(Err(_)) => return Ok(full_res("Invalid ASCII in header: X-GitHub-Event", StatusCode::BAD_REQUEST)),
        None => return Ok(full_res("Missing header: X-GitHub-Event", StatusCode::BAD_REQUEST)),
    };

    // Read entire body into `Bytes`. We have to set an upper limit to protect the server from
    // massive allocations.
    let upper = body.size_hint().upper().unwrap_or(u64::MAX);
    if upper > 1024 * 64 {
        eprintln!("Rejecting request because payload is too large.");
        return Ok(full_res("Body too big", StatusCode::PAYLOAD_TOO_LARGE));
    }
    let body = body.collect().await?.to_bytes();

    // Now that we have read the entire body, we should validate the signature before proceeding.
    if !validate_request(&config.secret, &head.headers, &body) {
        eprintln!("Rejecting request becuase signature is missing or invaldi");
        return Ok(full_res("Missing or invalid signature", StatusCode::BAD_REQUEST));
    }

    for command in &config.commands {
        if command.event == event {
            let command_clone = command.clone();
            let body_clone = body.clone();
            tokio::spawn(async move {
                match run_command(&command_clone, body_clone.as_ref()).await {
                    Ok(s) => match s.code() {
                        Some(code) => println!("Command finished with exit code {}: {:?}", code, command_clone),
                        None => println!("Command finished without exit code: {:?}", command_clone),
                    },
                    Err(e) => eprintln!("Failed to spawn command: {:?}\nerror: {}", command_clone, e),
                }
            });
        }
    }

    Ok(empty_res(StatusCode::NO_CONTENT))
}

async fn run_command(command: &config::Command, body: &[u8]) -> io::Result<ExitStatus> {
    let mut child = Command::new(&command.command)
        .stdin(Stdio::piped())    // We will feed the event data through stdin.
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .args(&command.args)
        .spawn()?;

    // Feed data through stdin. Sure hope whatever a "deadlock" is doesn't happen here.
    let mut child_stdin = child.stdin.take().expect("child has stdin");
    child_stdin.write_all(body).await?;
    drop(child_stdin);

    Ok(child.wait().await?)
}

/// Utility to create an empty response.
fn empty_res(status: StatusCode) -> Response<BoxBody<Bytes, hyper::Error>> {
    let body = Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed();

    let mut response = Response::new(body);
    *response.status_mut() = status;
    response
}

/// Utility to create a full (i.e. with content) response.
fn full_res<T: Into<Bytes>>(
    chunk: T,
    status: StatusCode,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let body = Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed();

    let mut response = Response::new(body);
    *response.status_mut() = status;
    response
}

/// Decodes a string slice into a string of bytes.
///
/// Implementation taken from [this stackoverflow post](https://stackoverflow.com/a/52992629).
fn decode_hex(s: &str) -> Result<Vec<u8>, ParseIntError> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect()
}

/// Validates the signature that GitHub attaches to events.
fn validate_request(secret: &String, headers: &HeaderMap<HeaderValue>, body: &Bytes) -> bool {
    // To verify the authenticity of the event, GitHub attaches a signature of the payload to
    // every request. We extract the header. The header value will look something like this:
    //
    //     x-hub-signature-256: sha256=6803d2a3e495fc4bd286d428ea4b794476a1ff1b72bbea4dfafd2477d5d89188
    let maybe_signature = headers
        .get("x-hub-signature-256")
        .and_then(|hv| hv.to_str().ok())         // HeaderValue => &str
        .and_then(|s| s.strip_prefix("sha256=")) // sha256=2843i4aklds... => 2843i4aklds...
        .and_then(|s| decode_hex(s).ok());       // &str -> vec<u8>
    let signature = match maybe_signature {
        Some(s) => s,
        None => return false, // Missing or invalid signature
    };

    // Now we independantly calculate a signature of the payload we just read, using the secret. If
    // Github computed the signature with the same secret, we should be all good.
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(&body);
    mac.verify_slice(&signature).is_ok()
}
