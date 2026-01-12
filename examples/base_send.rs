use std::time::Duration;

use clap::Parser;
use thubo::{Bytes, CongestionControl, Priority, QoS};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    let Args { addr, interval } = Args::parse();

    // Connect to TCP socket
    let stream = TcpStream::connect(addr).await.unwrap();
    let (_stream_reader, stream_writer) = stream.into_split();

    // Create thubo sender and spawn a sender_task
    let qos = QoS::default()
        .with_priority(Priority::Medium)
        .with_congestion_control(CongestionControl::Block)
        .with_express(false);
    let (mut sender, _sender_task) = thubo::sender(stream_writer).qos(qos).build();

    // Create a reusable buffer for serialization
    let mut interval = tokio::time::interval(Duration::from_secs_f32(interval));
    for counter in 0..=u32::MAX {
        interval.tick().await;

        sender
            .send(Bytes::from(format!("[{:4?}] Hello, World!", counter)))
            .await
            .unwrap();
    }
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    addr: String,
    #[arg(short, long, default_value = "1.0")]
    interval: f32,
}
