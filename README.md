[![CI](https://img.shields.io/github/actions/workflow/status/Mallets/thubo/ci.yaml?branch=main)](https://github.com/Mallets/thubo/actions?query=workflow:CI+branch:main)
[![docs.rs](https://img.shields.io/docsrs/thubo)](https://docs.rs/thubo/latest/thubo/)
[![Release](https://img.shields.io/crates/v/thubo)](https://crates.io/crates/thubo)
[![License](https://img.shields.io/badge/License-EPL%202.0-blue)](https://choosealicense.com/licenses/epl-2.0/)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

# Thubo

Thubo is a high-performance TX/RX network pipeline featuring strict priority scheduling, automatic batching, and message fragmentation. It’s designed for applications that require predictable, priority-based message delivery, even under heavy load.

This is especially useful for protocols prone to head-of-line blocking (e.g., TCP/TLS), where a single large, low-priority message might otherwise delay urgent messages.

## Why Thubo?

- ⚡ **Strict Priority Scheduling**: high-priority messages preempt lower-priority flows.
- 📦 **Automatic Batching**: maximizes throughput without manual tuning.
- 🔀 **Message Fragmentation**: prevents head-of-line blocking by splitting large messages.
- ⚙️ **Configurable Congestion Control**: do not block on data that may get stale.

## Overview

The diagram below illustrates the TX/RX network pipeline in operation, using all 4 priority queues (*High, Medium, Low, Background*).

```text
                                                              .....
 APPLICATION SEND                                     User code   :
┌─────────────┐  ┌────┐  ┌────────┐  ┌────┐  ┌────┐               :
│    B1       │  │ L1 │  │ M1     │  │ H1 │  │ H2 │               :
└──┬──────────┘  └─┬──┘  └──┬─────┘  └─┬──┘  └─┬──┘               :
  t0              t1       t2         t3      t4                  :
   ▼               ▼        ▼          ▼       ▼                  :
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~  :
 TX PIPELINE                                 Thubo code           :
┌──────────────────────────────────────────────────────────────┐  :
│  Queues:                                                     │  :
│   P0 (High):       [H1][H2]           ← t3 ← t4              │  :
│   P1 (Medium):     [M1a, M1b]         ← t2                   │  :
│   P2 (Low):        [L1a, L1b]         ← t1                   │  :
│   P3 (Background): [B1a, B1b, B1c]    ← t0                   │  :
|                                                              |  :
│              t0     t1   t2   t3 t4                          │  :
│  Pull Order: B1a → B1b → L1a → M1a → H1 H2 → M1b → L1b → B1c │  :
│                                                              │  :
│  TX Stream: [B1a][B1b][L1a][M1a][H1 H2][M1b][L1b][B1c]       │  :
└───────────┬──────────────────────────────────────────────────┘  :
            |                                                 .....
            ▼ Network
                                                              .....
 RX PIPELINE                                         Thubo code   :
┌──────────────────────────────────────────────────────────────┐  :
│  RX Stream: [B1a][B1b][L1a][M1a][H1 H2][M1b][L1b][B1c]       │  :
│                                                              │  :
│  Reassembled Messages: B1, L1, M1, H1, H2                    │  :
│                                                              │  :
│  Delivered by Priority: H1 → H2 → M1 → L1 → B1               │  :
└───────────┬──────────────────────────────────────────────────┘  :
            ▼                                                     :
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~  :
 APPLICATION RECEIVE                                  User code   :
┌────┐ ┌────┐ ┌────────┐ ┌────┐ ┌─────────────┐                   :
│ H1 │ │ H2 │ │ M1     │ │ L1 │ │    B1       │                   :
└────┘ └────┘ └────────┘ └────┘ └─────────────┘                   :
                                                              .....
```

See documentation for a more detailed explaination.

## Quick Start

```rust
use thubo::*;
use tokio::net::TcpStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a TCP connection
    let stream = TcpStream::connect("127.0.0.1:8080").await?;
    let (reader, writer) = stream.into_split();

    // Create bidirectional Thubo channel
    let (mut sender, sender_task) = thubo::sender(writer).build();
    let (mut receiver, receiver_task) = thubo::receiver(reader).build();

    // Send a message with default QoS
    sender.send(Bytes::from("my payload")).await?;

    // Receive messages in priority order
    let (msg, qos): (Bytes, QoS) = receiver.recv().await?;
    println!("Received message with QoS: {:?}", qos);

    Ok(())
}
```

## Performance

Thubo can batch tens of millions of small messages per second and saturate multi-gigabit networks.
The figure below shows the median throughput, with error bars representing the confidence interval, measured in messages per second (msg/s) and bits per second (bit/s).
The y-axis is logarithmic.

![plot](content/throughput.svg "Throughput test")

Thubo also achieves sub-millisecond latency, with ping times of a few tens of microseconds.
The figure below shows the median latency, with error bars indicating the confidence interval.
The y-axis is logarithmic.

![plot](content/latency.svg "Latency test")
