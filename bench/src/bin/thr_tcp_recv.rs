use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use clap::Parser;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let Args {
        addr,
        capacity,
        interval,
    } = Args::parse();

    // Bind TCP listener
    let listener = TcpListener::bind(addr).await.unwrap();
    // Accept connection
    let (stream, _addr) = listener.accept().await.unwrap();
    let (stream_reader, _stream_writer) = stream.into_split();

    // Create receiver and spawn a receiver task
    let (mut receiver, receiver_task) = thubo::receiver(stream_reader).capacity(capacity).build();

    static MSGS: AtomicUsize = AtomicUsize::new(0);
    tokio::spawn(thubo_bench::stats_loop_recv(
        receiver_task,
        Duration::from_secs_f32(interval),
        &MSGS,
    ));
    loop {
        let _ = receiver.recv().await.unwrap();
        MSGS.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    addr: String,
    #[arg(short, long, default_value = "16")]
    capacity: usize,
    #[arg(short, long, default_value = "1.0")]
    interval: f32,
}
