use carrier_breakpoint::web;
use carrier_breakpoint::vtime::FixedClock;
use carrier_breakpoint::Store;
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let mut addr: SocketAddr = "127.0.0.1:5348".parse().unwrap();
    let mut db = "carrier_breakpoint.db".to_string();
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
            other => eprintln!("ignoring unknown arg {other}"),
        }
    }
    let clock = FixedClock::seeded();
    let store = Store::open(&db, Box::new(clock)).expect("open sqlite database");
    let app = web::router(std::sync::Arc::new(store));
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind addr");
    println!("carrier-breakpoint listening on http://{addr} (db {db})");
    axum::serve(listener, app).await.expect("server");
}
