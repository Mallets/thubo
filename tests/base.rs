#![cfg(not(miri))]

use std::time::Duration;

use thubo::Bytes;
use tokio::net::{TcpListener, TcpStream};

const N: u64 = 1_000;

async fn pong(listener: TcpListener) {
    let (stream, _addr) = listener.accept().await.unwrap();
    let (stream_reader, stream_writer) = stream.into_split();

    let (mut sender, sender_task) = thubo::sender(stream_writer).build();
    let (mut receiver, reader_task) = thubo::receiver(stream_reader).build();

    // Echo messages until the client closes its write half.
    while let Ok((data, _)) = receiver.recv().await {
        sender.send(&data).await.unwrap();
    }
    sender.stop().await;
    drop(sender_task.stop().await.unwrap());
    receiver.stop().await;
    drop(reader_task.stop().await.unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn base() {
    tokio::time::timeout(Duration::from_secs(30), async {
        // An ephemeral port allows concurrent test runs without collisions.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let echo = tokio::spawn(pong(listener));

        let stream = TcpStream::connect(addr).await.unwrap();
        let (stream_reader, stream_writer) = stream.into_split();
        let (mut sender, sender_task) = thubo::sender(stream_writer).build();
        let (mut receiver, reader_task) = thubo::receiver(stream_reader).build();

        let mut chunks = Bytes::new();
        for value in 0..32u8 {
            chunks.push(vec![value; 8192].into());
        }
        // Empty, contiguous, multi-chunk and fragmented payloads.
        let mut payloads = vec![Bytes::new(), chunks];
        for size in [8, 32_000, 1_000_000] {
            payloads.push(Bytes::from((0..size).map(|i| (i % 251) as u8).collect::<Vec<_>>()));
        }
        for payload in payloads {
            let expected = payload.to_vec();
            for _ in 0..N {
                sender.send(&payload).await.unwrap();
                let (received, _) = receiver.recv().await.unwrap();
                assert_eq!(received.to_vec(), expected);
            }
        }

        // Send without waiting for echoes so multiple batches can queue up.
        // Receive concurrently to avoid backpressure deadlocking the echo loop.
        let producer = tokio::spawn(async move {
            for value in 0..N {
                let payload = Bytes::from(value.to_le_bytes().repeat(512));
                sender.send(&payload).await.unwrap();
            }
            // Keep the connection open until the client has checked every echo.
            sender
        });
        for value in 0..N {
            let (received, _) = receiver.recv().await.unwrap();
            assert_eq!(received.to_vec(), value.to_le_bytes().repeat(512));
        }
        let sender = producer.await.unwrap();
        sender.stop().await;
        drop(sender_task.stop().await.unwrap());
        receiver.stop().await;
        drop(reader_task.stop().await.unwrap());
        echo.await.unwrap();
    })
    .await
    .expect("TCP round-trip and ordering test timed out");
}
