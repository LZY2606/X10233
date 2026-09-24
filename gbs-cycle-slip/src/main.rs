//! 载波断点台：GNSS 周跳候选、多频组合与弧段修订史。
//! 本地优先：SQLite 文件在数据目录内，无公网依赖。

mod api;
mod bundle;
mod detector;
mod domain;
mod engine;
mod ids;
mod parser;
mod sample;
mod storage;
mod time;

use engine::AppState;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_http::services::ServeDir;

#[derive(Debug)]
struct Args {
    addr: String,
    data_dir: String,
}

fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut addr = "127.0.0.1:5348".to_string();
    let mut data_dir = "data".to_string();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--addr" => addr = args.next().expect("--addr 需要值"),
            "--data-dir" => data_dir = args.next().expect("--data-dir 需要值"),
            "--help" | "-h" => {
                println!("用法: gbs-cycle-slip [--addr 127.0.0.1:5348] [--data-dir ./data]");
                std::process::exit(0);
            }
            other => {
                eprintln!("未知参数: {other}");
                std::process::exit(2);
            }
        }
    }
    Args { addr, data_dir }
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    let dir = PathBuf::from(&args.data_dir);
    std::fs::create_dir_all(&dir).expect("创建数据目录失败");
    let db_path = dir.join("gbs-cycle-slip.sqlite");
    let store = storage::Store::open(&db_path).expect("打开 SQLite 失败");
    let state = Arc::new(AppState {
        store: Mutex::new(store),
        clock: Box::new(time::SystemClock),
        data_dir: args.data_dir.clone(),
    });

    let static_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static");
    let serve = ServeDir::new(&static_dir)
        .fallback(tower_http::services::ServeFile::new(static_dir.join("index.html")));

    let app = api::router(state).fallback_service(serve);
    let addr: SocketAddr = args.addr.parse().expect("无法解析监听地址");
    println!("载波断点台已启动: http://{addr}");
    println!("数据目录: {}", dir.display());
    let listener = tokio::net::TcpListener::bind(addr).await.expect("绑定地址失败");
    axum::serve(listener, app).await.expect("服务失败");
}
