//! TCP transport tests for vectored (writev) batch writes.
//!
//! Exercises the iovec gather path: multi-chunk payloads, coalesced
//! multi-batch writes, and partial vectored writes on a small send buffer.

use std::{
    net::{TcpListener, TcpStream},
    time::Duration,
};

use thubo::Bytes;
use tokio::io::AsyncReadExt;

fn sockets() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    client.set_nodelay(true).unwrap();
    server.set_nodelay(true).unwrap();
    (client, server)
}

fn tokio_socket(socket: TcpStream) -> tokio::net::TcpStream {
    socket.set_nonblocking(true).unwrap();
    tokio::net::TcpStream::from_std(socket).unwrap()
}

fn payloads() -> Vec<Bytes> {
    let mut chunks = Bytes::new();
    for value in 0..32u8 {
        chunks.push(vec![value; 8192].into());
    }
    vec![
        Bytes::new(),
        Bytes::from(vec![7; 1]),
        Bytes::from(vec![9; 4096]),
        // Multi-chunk payload: forces several iovecs per message.
        chunks,
        // Larger than one batch: fragmented across multiple batches.
        Bytes::from(vec![11; 1_000_000]),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vectored_roundtrip_delivers_all_payload_shapes() {
    let (tx, rx) = sockets();
    let (mut sender, task) = thubo::sender(tokio_socket(tx)).capacity(2).build();
    let (mut receiver, reader_task) = thubo::receiver(tokio_socket(rx)).capacity(2).build();

    let expected = payloads();
    let sent = expected.clone();
    let producer = tokio::spawn(async move {
        for payload in sent {
            sender.send(&payload).await.unwrap();
        }
        sender.stop().await;
    });

    for payload in expected {
        let received = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
            .await
            .expect("receiving payload")
            .unwrap();
        assert_eq!(received.0.to_vec(), payload.to_vec());
    }
    producer.await.unwrap();

    drop(task.stop().await.unwrap());
    receiver.stop().await;
    drop(reader_task.stop().await.unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn coalesced_batches_preserve_message_order() {
    let (tx, rx) = sockets();
    let (mut sender, task) = thubo::sender(tokio_socket(tx))
        .batch_size(1024)
        .timeout_batch(Duration::from_micros(100))
        .build();
    let (mut receiver, reader_task) = thubo::receiver(tokio_socket(rx)).build();

    // Many small messages: several batches complete together and the
    // transport gathers them into one vectored write.
    let producer = tokio::spawn(async move {
        for value in 0..10_000u32 {
            let payload = Bytes::from(value.to_le_bytes().to_vec());
            sender.send(&payload).await.unwrap();
        }
        sender.stop().await;
    });

    for value in 0..10_000u32 {
        let received = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
            .await
            .expect("receiving message")
            .unwrap();
        assert_eq!(received.0.to_vec(), value.to_le_bytes().to_vec());
    }
    producer.await.unwrap();

    drop(task.stop().await.unwrap());
    receiver.stop().await;
    drop(reader_task.stop().await.unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partial_vectored_writes_complete_on_small_send_buffer() {
    use std::os::fd::AsRawFd;

    let (tx, rx) = sockets();
    // A tiny send buffer forces writev to return partial writes, driving
    // the slice-advance loop across many iterations.
    let size: libc::c_int = 4096;
    assert_eq!(
        unsafe {
            libc::setsockopt(
                tx.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&size as *const libc::c_int).cast(),
                std::mem::size_of_val(&size) as libc::socklen_t,
            )
        },
        0
    );

    let (mut sender, task) = thubo::sender(tokio_socket(tx))
        .capacity(2)
        .timeout(Duration::from_secs(10))
        .build();
    let (mut receiver, reader_task) = thubo::receiver(tokio_socket(rx)).build();

    let mut chunks = Bytes::new();
    for value in 0..32u8 {
        chunks.push(vec![value; 8192].into());
    }
    let payload = chunks;
    sender.send(&payload).await.unwrap();
    sender.stop().await;

    let received = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
        .await
        .expect("receiving multi-chunk payload")
        .unwrap();
    assert_eq!(received.0.to_vec(), payload.to_vec());

    drop(task.stop().await.unwrap());
    receiver.stop().await;
    drop(reader_task.stop().await.unwrap());
}

#[tokio::test]
async fn keep_alive_is_a_zero_length_batch() {
    let (tx, peer) = sockets();
    let (sender, task) = thubo::sender(tokio_socket(tx)).build();
    task.set_keep_alive(Duration::from_millis(10));
    let mut peer = tokio_socket(peer);
    for _ in 0..2 {
        let mut header = [1u8; 2];
        tokio::time::timeout(Duration::from_secs(1), peer.read_exact(&mut header))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(header, [0, 0]);
    }
    sender.stop().await;
    drop(task.stop().await.unwrap());
}
