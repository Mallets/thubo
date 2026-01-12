use clap::Parser;
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
        println!("{}\t{:?}", String::from_utf8(bytes.to_vec()).unwrap(), qos);
    }
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    addr: String,
}
