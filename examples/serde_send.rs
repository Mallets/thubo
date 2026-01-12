use std::time::Duration;

use clap::Parser;
use serde::{Deserialize, Serialize};
use thubo::Bytes;
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    let Args { addr, interval } = Args::parse();

    // Connect to TCP socket
    let stream = TcpStream::connect(addr).await.unwrap();
    let (_stream_reader, stream_writer) = stream.into_split();

    // Create thubo sender and spawn a sender_task
    let (mut sender, _sender_task) = thubo::sender(stream_writer).build();

    let mut interval = tokio::time::interval(Duration::from_secs_f32(interval));
    for counter in 0..=u32::MAX {
        interval.tick().await;

        // Create a message to send
        let value = Foo {
            counter,
            value: "Hello, World!",
        };

        // Serialize the value to bytes
        let mut bytes = Bytes::new();
        serde_json::to_writer(&mut bytes.writer(), &value).unwrap();

        // Send the message
        sender.send(bytes).await.unwrap();
    }
}

#[derive(Serialize, Deserialize)]
struct Foo {
    counter: u32,
    value: &'static str,
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    addr: String,
    #[arg(short, long, default_value = "1.0")]
    interval: f32,
}
