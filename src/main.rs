use cbs::store::Store;
use cbs::vclock::VClock;
use cbs::web::app;
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let mut addr: SocketAddr = "127.0.0.1:5348".parse().unwrap();
    let mut db = "cbs_data/cbs.sqlite".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--addr" => {
                if let Some(v) = args.next() {
                    addr = v.parse().expect("valid socket addr");
                }
            }
            "--db" => {
                if let Some(v) = args.next() {
                    db = v;
                }
            }
            other => eprintln!("ignoring unknown argument: {other}"),
        }
    }
    if let Some(parent) = std::path::Path::new(&db).parent() {
        std::fs::create_dir_all(parent).expect("create data dir");
    }
    let store = Arc::new(Store::open(&db).expect("open sqlite database"));
    let clock = VClock::default();
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind address");
    println!("Carrier Break Station listening on http://{addr} (db: {db})");
    axum::serve(listener, app(store, clock)).await.expect("server");
}
