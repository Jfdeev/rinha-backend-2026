mod data;
mod parse;
mod vectorize;

use data::Database;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

async fn handle(
    db: Arc<Database>,
    req: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    // Clone method/path before potentially consuming req.
    let method = req.method().clone();
    let path = req.uri().path().to_owned();

    Ok(match (method, path.as_str()) {
        (Method::GET, "/ready") => Response::builder()
            .status(StatusCode::OK)
            .body(Full::new(Bytes::from_static(b"ok")))
            .unwrap(),

        (Method::POST, "/fraud-score") => {
            let body = match req.into_body().collect().await {
                Ok(b) => b.to_bytes(),
                Err(_) => return Ok(fallback_response()),
            };
            score_response(&body, &db)
        }

        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::new()))
            .unwrap(),
    })
}

// Pre-templated bodies indexed by fraud_count (0..=5).
// Length is constant (38 bytes) so we can serve them all the same way.
//                                              "{"approved":XXXXX,"fraud_score":Y.YYYY}"
const BODY_0: &[u8] = br#"{"approved":true ,"fraud_score":0.0000}"#;
const BODY_1: &[u8] = br#"{"approved":true ,"fraud_score":0.2000}"#;
const BODY_2: &[u8] = br#"{"approved":true ,"fraud_score":0.4000}"#;
const BODY_3: &[u8] = br#"{"approved":false,"fraud_score":0.6000}"#;
const BODY_4: &[u8] = br#"{"approved":false,"fraud_score":0.8000}"#;
const BODY_5: &[u8] = br#"{"approved":false,"fraud_score":1.0000}"#;
const BODIES: [&[u8]; 6] = [BODY_0, BODY_1, BODY_2, BODY_3, BODY_4, BODY_5];

#[inline(always)]
fn body_for(fraud_count: u8) -> &'static [u8] {
    BODIES[fraud_count.min(5) as usize]
}

fn score_response(body: &[u8], db: &Database) -> Response<Full<Bytes>> {
    // try_score now returns the fraud_count directly (0..=5); on parse failure
    // we fall back to "approved=true, fraud_score=0" (BODY_0).
    let fraud_count = try_score(body, db).unwrap_or(0);
    Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from_static(body_for(fraud_count))))
        .unwrap()
}

fn fallback_response() -> Response<Full<Bytes>> {
    Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from_static(BODY_0)))
        .unwrap()
}

fn try_score(body: &[u8], db: &Database) -> Option<u8> {
    let payload = parse::parse(body)?;
    let vec = vectorize::vectorize(&payload);
    Some(db.knn_fraud_count(&vec))
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let data_path =
        std::env::var("DATA_PATH").unwrap_or_else(|_| "/app/data/refs.bin".to_string());

    eprintln!("[main] loading database from {data_path}");
    let db = Arc::new(Database::load(&data_path));
    eprintln!("[main] database ready, listening on :9999");

    let addr: SocketAddr = "0.0.0.0:9999".parse().unwrap();
    let listener = TcpListener::bind(addr).await.unwrap();

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[main] accept error: {e}");
                continue;
            }
        };
        let io = TokioIo::new(stream);
        let db = Arc::clone(&db);
        tokio::spawn(async move {
            let svc = service_fn(move |req| {
                let db = Arc::clone(&db);
                async move { handle(db, req).await }
            });
            if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
                eprintln!("[conn] error: {e}");
            }
        });
    }
}
