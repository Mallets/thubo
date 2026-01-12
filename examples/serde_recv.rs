use clap::Parser;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let Args { addr } = Args::parse();

    // Bind TCP listener
    let listener = TcpListener::bind(addr).await.unwrap();
    // Accept connection
    let (stream, _addr) = listener.accept().await.unwrap();
    let (stream_reader, _stream_writer) = stream.into_split();

    // Create receiver and spawn a receiver task
    let (mut receiver, _receiver_task) = thubo::receiver(stream_reader).build();
    loop {
        // Receive the next message from the network (delivered in priority order)
        let (bytes, qos) = receiver.recv().await.unwrap();
        // Deserialize the JSON message directly from the Bytes reader
        let v: Foo = serde_json::from_reader(bytes.reader()).unwrap();
        println!("[{:4?}] {}\t{:?}", v.counter, v.value, qos);
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Foo {
    counter: u32,
    value: String,
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    addr: String,
}
