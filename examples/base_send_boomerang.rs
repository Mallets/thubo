use std::{io::Write, time::Duration};

use clap::Parser;
use thubo::{Bytes, collections::Boomerang};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    let Args { addr, interval } = Args::parse();

    // Connect to TCP socket
    let stream = TcpStream::connect(addr).await.unwrap();
    let (_stream_reader, stream_writer) = stream.into_split();

    // Create thubo sender and spawn a sender_task
    let (mut sender, _sender_task) = thubo::sender(stream_writer).build();

    // Create a reusable buffer for serialization
    let mut buffer = Vec::new();
    let mut interval = tokio::time::interval(Duration::from_secs_f32(interval));
    for counter in 0..=u32::MAX {
        interval.tick().await;

        // Serialize the value into the buffer
        buffer
            .write_fmt(format_args!("[{:4?}] Hello, World!", counter))
            .unwrap();

        // Wrap the buffer in a Boomerang to track when it is dropped by the TX pipeline.
        // When it is dropped, the allocated buffer can be hence retrieved and reused.
        // By doing so, there is no need to allocated a new buffer every time.
        let (bytes, boomerang) = Boomerang::new(buffer);

        // Send the message
        sender.send(Bytes::from(bytes)).await.unwrap();

        // Wait for the buffer to be returned after transmission completes
        buffer = boomerang.await.unwrap();
        // Clear the buffer for the next message
        buffer.clear();
    }
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    addr: String,
    #[arg(short, long, default_value = "1.0")]
    interval: f32,
}
