//! Thubo: a high-performance TX/RX network pipeline featuring strict priority
//! scheduling, automatic batching, and fragmentation.
//!
//! Thubo is designed for applications that demand strict, priority-based
//! message delivery with configurable congestion control. By managing
//! independent priority queues and fragmenting messages within each queue,
//! Thubo enables high-priority traffic to seamlessly interleave and preempt
//! lower-priority flows, ensuring responsive behavior even under heavy load.
//!
//! The priority-based interleaving is particularly valuable for protocols that
//! experience head-of-line blocking (such as TCP/TLS), where a single large
//! low-priority message could otherwise delay urgent high-priority traffic.
//!
//! # Overview
//!
//! The diagram below illustrates the TX/RX network pipeline in operation, using
//! all 4 priority queues (High, Medium, Low, Background).
//!
//! ```text
//!                                                               .....
//!  APPLICATION SEND                                     User code   :
//! ┌─────────────┐  ┌────┐  ┌────────┐  ┌────┐  ┌────┐               :
//! │    B1       │  │ L1 │  │ M1     │  │ H1 │  │ H2 │               :
//! └──┬──────────┘  └─┬──┘  └──┬─────┘  └─┬──┘  └─┬──┘               :
//!   t0              t1       t2         t3      t4                  :
//!    ▼               ▼        ▼          ▼       ▼                  :
//! ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~  :
//!  TX PIPELINE                                 Thubo code           :
//! ┌──────────────────────────────────────────────────────────────┐  :
//! │  Queues:                                                     │  :
//! │   P0 (High):       [H1][H2]           ← t3 ← t4              │  :
//! │   P1 (Medium):     [M1a, M1b]         ← t2                   │  :
//! │   P2 (Low):        [L1a, L1b]         ← t1                   │  :
//! │   P3 (Background): [B1a, B1b, B1c]    ← t0                   │  :
//! |                                                              |  :
//! │              t0     t1   t2   t3 t4                          │  :
//! │  Pull Order: B1a → B1b → L1a → M1a → H1 H2 → M1b → L1b → B1c │  :
//! │                                                              │  :
//! │  TX Stream: [B1a][B1b][L1a][M1a][H1 H2][M1b][L1b][B1c]       │  :
//! └───────────┬──────────────────────────────────────────────────┘  :
//!             |                                                 .....
//!             ▼ Network
//!                                                               .....
//!  RX PIPELINE                                         Thubo code   :
//! ┌──────────────────────────────────────────────────────────────┐  :
//! │  RX Stream: [B1a][B1b][L1a][M1a][H1 H2][M1b][L1b][B1c]       │  :
//! │                                                              │  :
//! │  Reassembled Messages: B1, L1, M1, H1, H2                    │  :
//! │                                                              │  :
//! │  Delivered by Priority: H1 → H2 → M1 → L1 → B1               │  :
//! └───────────┬──────────────────────────────────────────────────┘  :
//!             ▼                                                     :
//! ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~  :
//!  APPLICATION RECEIVE                                  User code   :
//! ┌────┐ ┌────┐ ┌────────┐ ┌────┐ ┌─────────────┐                   :
//! │ H1 │ │ H2 │ │ M1     │ │ L1 │ │    B1       │                   :
//! └────┘ └────┘ └────────┘ └────┘ └─────────────┘                   :
//!                                                               .....
//! ```
//!
//! At the *Application Send* stage, five messages (*B1*, *L1*, *M1*, *H1*, and *H2*) are pushed into the system at
//! different times (*t0*, *t1*, *t2*, *t3*, and *t4* respectively). These messages enter the *TX Pipeline*, where they
//! may be fragmented if too large and assigned to queues based on their priority.
//!
//! The highest priority queue, *P0 (High)*, contains the non-fragmented messages *H1* and *H2*, while the medium
//! priority queue, *P1 (Medium)*, holds the fragments *M1a* and *M1b*. The low priority queue, *P2 (Low)*, contains
//! the fragments *L1a* and *L1b*. The lowest priority queue, *P3 (Background)*, contains the fragments *B1a*, *B1b*,
//! and *B1c*.
//!
//! The *TX Pipeline* schedules and pulls fragments in a specific order: *B1a*,
//! *B1b*, *L1a*, *M1a*, then batches *H1* and *H2* together, followed by *M1b*,
//! *L1b*, and *B1c*. When high-priority messages *H1* and *H2* arrive, they
//! preempt the transmission of lower-priority fragments, according to the strict
//! priority scheduling. The batched high-priority messages *[H1 H2]* are
//! transmitted together as a single unit over the *Network* to the *RX
//! Pipeline*.
//!
//! Upon arrival, the *RX Pipeline* receives the fragments in the same order
//! they were sent: *B1a*, *B1b*, *L1a*, *M1a*, *[H1 H2]*, *M1b*, *L1b*, and
//! *B1c*. The fragments are reassembled into their original messages: *B1*,
//! *L1*, *M1*, *H1*, and *H2*.
//!
//! However, the delivery to the *Application Receive* follows the priority
//! order, with *H1* delivered first, followed by *H2*, then *M1*, *L1*, and
//! finally *B1*. This process ensures that higher-priority messages are
//! delivered before lower-priority ones, even if they arrive later in the
//! stream.
//!
//! # Features
//!
//! - **4-level Priority System**: message ordering with strict priority scheduling (see [`Priority`])
//! - **Congestion Control**: choose between blocking ([Block](`CongestionControl::Block`)) or dropping messages
//!   ([Drop](`CongestionControl::Drop`)) under load
//! - **Express Delivery**: send urgent messages immediately without batching for lowest latency
//! - **Automatic Batching**: small messages are transparently aggregated for highest throughput
//! - **Automatic Fragmentation**: large messages are transparently fragmented and reassembled, allowing higher priority
//!   messages to interleave and preempt ongoing transmissions
//! - **Zero-Copy Buffers**: efficient buffer management with [`Chunk`] and [`Bytes`] types
//!
//! # Quick Start
//!
//! ```no_run
//! use thubo::*;
//! use tokio::net::TcpStream;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create a TCP connection
//!     let stream = TcpStream::connect("127.0.0.1:8080").await?;
//!     let (reader, writer) = stream.into_split();
//!
//!     // Create bidirectional channel with buffer capacity and batch size
//!     let (mut sender, sender_task) = thubo::sender(writer).build();
//!     let (mut receiver, receiver_task) = thubo::receiver(reader).build();
//!
//!     // Set the QoS to send messages with high-priority
//!     sender.qos(
//!         QoS::default()
//!             .with_priority(Priority::High)
//!             .with_congestion_control(CongestionControl::Block),
//!     );
//!     sender.send(&Bytes::from("urgent message")).await?;
//!
//!     // Receive messages in priority order
//!     let (msg, qos): (Bytes, QoS) = receiver.recv().await?;
//!     println!("Received message with priority: {:?}", qos.priority());
//!
//!     Ok(())
//! }
//! ```
//!
//! # Buffer management
//!
//! Thubo provides efficient buffer management through two key types: [`Chunk`] and [`Bytes`].
//!
//! ## Chunk
//!
//! [`Chunk`] is a reference-counted, immutable buffer that supports cheap cloning and view creation:
//!
//! ```
//! use thubo::Chunk;
//!
//! // Create a chunk from data
//! let data = vec![1, 2, 3, 4, 5];
//! let chunk: Chunk = data.into();
//!
//! // Cheap clone - shares the underlying buffer
//! let chunk2 = chunk.clone();
//!
//! // Create a view into a portion of the buffer (zero-copy)
//! let view = chunk.view(1..4).unwrap();
//! assert_eq!(view.as_slice(), &[2, 3, 4]);
//! ```
//!
//! ## Bytes
//!
//! [`Bytes`] is a collection of one or more [`Chunk`]s that can represent non-contiguous memory:
//!
//! ```
//! use thubo::Bytes;
//!
//! let mut bytes = Bytes::new();
//! bytes.push("Hello ".as_bytes().to_vec().into());
//! bytes.push("World!".as_bytes().to_vec().into());
//!
//! // Bytes provides Read, Seek, and Write interface
//! use std::io::Read;
//! let mut buf = vec![0u8; 12];
//! let mut reader = bytes.reader();
//! reader.read_exact(&mut buf).unwrap();
//! ```
//!
//! ## Batching
//!
//! When **batching** multiple small messages, Thubo copies data into a single contiguous memory
//! region before writing to the network. This optimization reduces system call overhead:
//!
//! ```text
//! Message 1: [A A A]
//! Message 2: [B B]           Copy into      Batch Buffer:
//! Message 3: [C C C C]   →   single buffer → [A A A|B B|C C C C]
//!                                                      ↓
//!                                            Single write() syscall
//! ```
//!
//! This copy is necessary to achieve maximum throughput by minimizing the number of network writes.
//!
//! ## Fragmentation
//!
//! When **fragmenting** large messages, Thubo uses zero-copy [`Bytes`] views. No data is copied;
//! instead, partial views of the buffer are created and sent separately:
//!
//! ```text
//! Original Buffer: [▥▥▥▥▥▥▥▥▥▥▥▥▥▥▥▥▥▥▥▥▥▥▥▥]
//!                        ↓ Create views (zero-copy)
//! Fragment 1:      [▥▥▥▥▥▥▥▥]
//! Fragment 2:              [▥▥▥▥▥▥▥▥]
//! Fragment 3:                      [▥▥▥▥▥▥▥▥]
//!                        ↓
//!                  Each fragment sent as separate write()
//! ```
//!
//! This enables high-priority messages to interleave between fragments without copying data.
//!
//! ## Buffer reuse
//!
//! The [`Boomerang`](`crate::collections::Boomerang`) type allows you to get back the underlying buffer once it's no
//! longer needed, enabling buffer reuse and reducing allocations.
//!
//! When [`send()`](`Sender::send`) returns, your message has been queued in the TX pipeline but **it may not yet be
//! written to the network**. A background task asynchronously handles batching, fragmentation, and network
//! transmission. The [`Boomerang`](`crate::collections::Boomerang`) future completes only after the data has been fully
//! transmitted, at which point you can safely reclaim and reuse the buffer.
//!
//! ```no_run
//! use thubo::{Bytes, collections::Boomerang};
//! use tokio::net::TcpStream;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create a TCP connection
//!     let stream = TcpStream::connect("127.0.0.1:8080").await?;
//!     let (reader, writer) = stream.into_split();
//!
//!     // Create bidirectional channel with buffer capacity and batch size
//!     let (mut sender, sender_task) = thubo::sender(writer).build();
//!     let (mut receiver, receiver_task) = thubo::receiver(reader).build();
//!
//!     let mut buffer = vec![42u8; 42];
//!     loop {
//!         let (bytes, boomerang) = Boomerang::new(buffer);
//!         sender.send(Bytes::from(bytes)).await?; // Pass the ownership to not keep any reference
//!         // Wait for the buffer to return and ready to be reused
//!         buffer = boomerang.await.unwrap();
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! # Performance Considerations
//!
//! ## Batching
//!
//! Messages are batched to improve throughput by amortizing system call overhead
//! and header costs. Thubo provides two knobs to control batching behavior:
//!
//! ### Batch Size
//!
//! Configure via [`SenderBuilder::batch_size()`]. Controls the maximum size of
//! each batch. **Most applications should use the default and rarely need to
//! modify this setting.**
//!
//! You may want to adjust batch size only when:
//! - **Matching MTU**: Set batch size to match your network's Maximum Transmission Unit to avoid IP fragmentation
//! - **Controlling head-of-line blocking**: Smaller batches allow higher priority messages to preempt lower priority
//!   traffic more frequently, reducing the maximum latency for urgent messages
//!
//! Trade-offs:
//! - **Smaller batches** (1-4KB): Lower latency, more frequent preemption opportunities, more system call overhead
//! - **Larger batches** (8-64KB): Higher throughput, less preemption flexibility, slightly higher latency
//! - **Maximum**: 64KB (protocol limit - batch size is represented on the wire as an unsigned 16-bit integer)
//!
//! **Note**: Moving to 32-bit batch sizes would be inadvisable in any case. Larger batches would:
//! - Significantly worsen head-of-line blocking, as lower priority messages would block high priority traffic for
//!   longer periods
//! - Increase network transmission delay, causing unacceptable latency for interactive applications
//! - Reduce the effectiveness of priority-based preemption, which is a core design goal of Thubo
//!
//! ### Batch Timeout
//!
//! Configure via [`SenderBuilder::timeout_batch()`]. Controls how long a batch
//! can remain uncommitted before being forcibly flushed:
//! - **Lower values** (1-10μs): Reduces latency for sparse traffic, more frequent small batches
//! - **Higher values** (100μs-1ms): Better throughput for burst traffic, may increase latency
//! - **Default**: 10μs
//!
//! The pipeline automatically flushes batches when they stop growing, preventing
//! idle batches from waiting unnecessarily.
//!
//! ### Express Mode
//!
//! Set via [`QoS::express()`]. Bypasses batching entirely for critical messages
//! that require minimum latency, sending them immediately regardless of batch
//! state.
//!
//! ## Priority Starvation
//!
//! Thubo uses **strict priority** scheduling: higher priority messages are
//! always processed first. Sustained high-priority traffic can starve lower
//! priorities. Design your priority usage accordingly:
//! - Use [`Priority::High`] sparingly for truly critical messages
//! - Most traffic should use [`Priority::Medium`] (default)
//! - Use [`Priority::Low`] for non time-sensitive data
//! - Reserve [`Priority::Background`] for non-critical bulk transfers
//!
//! # Acknowledgements
//!
//! The design of Thubo is inspired by the transmission pipeline architecture of
//! [Eclipse Zenoh](https://zenoh.io). The author of Thubo is the original author
//! of the transmission pipeline in Zenoh.
mod api;
mod buffers;
mod codec;
pub mod collections;
mod pipeline;
mod protocol;
mod sync;

pub use api::*;
pub use buffers::{bytes::*, chunk::*};
pub use pipeline::{rx::RecvError, tx::SendError};
pub use protocol::core::{CongestionControl, Priority, QoS};
