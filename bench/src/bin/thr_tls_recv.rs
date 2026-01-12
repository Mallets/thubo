use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use clap::Parser;
use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

#[tokio::main]
async fn main() {
    let Args {
        addr,
        capacity,
        interval,
    } = Args::parse();

    // TLS config
    let certs = CertificateDer::pem_slice_iter(SERVER_CERT.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_slice(SERVER_KEY.as_bytes()).unwrap();
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));

    // Bind TCP listener
    let listener = TcpListener::bind(addr).await.unwrap();
    // Accept TCP connection
    let (stream, _addr) = listener.accept().await.unwrap();
    // Accept TLS connection
    let stream = acceptor.accept(stream).await.unwrap();

    // Split the TLS stream into reader and writer
    let (stream_reader, _stream_writer) = tokio::io::split(stream);
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

const SERVER_KEY: &str = "-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEAmDCySqKHPmEZShDH3ldPaV/Zsh9+HlHFLk9H10vJZj5WfzVu
5puZQ8GvBFIOtVrl0L9qLkA6bZiHHXm/8OEVvd135ZMp4NV23fdTsEASXfvGVQY8
y+4UkZN0Dw6sfwlQVPyNRplys2+nFs6tX05Dp9VizV39tSOqe/jd6hyzxSUHqFat
RwQRXAI04CZ6ckDb0Riw7i0yvjrFhBom9lPKq4IkXZGgS5MRl0pRgAZTqHEMlv8z
oX+KcG9mfyQIHtpkVuSHHsQjwVop7fMnT7KCQ3bPI+fgMmAg+h1IR19Dm0JM+9zl
u39j0IbkytrsystGM+pTRbdp7s2lgtOMCFt0+wIDAQABAoIBADNTSO2uvlmlOXgn
DKDJZTiuYKaXxFrJTOx/REUxg+x9XYJtLMeM9jVJnpKgceFrlFHAHDkY5BuN8xNX
ugmsfz6W8BZ2eQsgMoRNIuYv1YHopUyLW/mSg1FNHzjsw/Pb2kGvIp4Kpgopv3oL
naCkrmBtsHJ+Hk/2hUpl9cE8iMwVWcVevLzyHi98jNy1IDdIPhRtl0dhMiqC5MRr
4gLJ5gNkLYX7xf3tw5Hmfk/bVNProqZXDIQVI7rFvItX586nvQ3LNQkmW/D2ShZf
3FEqMu6EdA2Ycc4UZgAlQNGV0VBrWWVXizOQ+9gjLnBk3kJjqfigCU6NG94bTJ+H
0YIhsGECgYEAwdSSyuMSOXgzZQ7Vv+GsNn/7ivi/H8eb/lDzksqS/JroA2ciAmHG
2OF30eUJKRg+STqBTpOfXgS4QUa8QLSwBSnwcw6579x9bYGUhqD2Ypaw9uCnOukA
CwwggZ9cDmF0tb5rYjqkW3bFPqkCnTGb0ylMFaYRhRDU20iG5t8PQckCgYEAyQEM
KK18FLQUKivGrQgP5Ib6IC3myzlHGxDzfobXGpaQntFnHY7Cxp/6BBtmASzt9Jxu
etnrevmzrbKqsLTJSg3ivbiq0YTLAJ1FsZrCp71dx49YR/5o9QFiq0nQoKnwUVeb
/hrDjMAokNkjFL5vouXO711GSS6YyM4WzAKZAqMCgYEAhqGxaG06jmJ4SFx6ibIl
nSFeRhQrJNbP+mCeHrrIR98NArgS/laN+Lz7LfaJW1r0gIa7pCmTi4l5thV80vDu
RlfwJOr4qaucD4Du+mg5WxdSSdiXL6sBlarRtVdMaMy2dTqTegJDgShJLxHTt/3q
P0yzBWJ5TtT3FG0XDqum/EkCgYAYNHwWWe3bQGQ9P9BI/fOL/YUZYu2sA1XAuKXZ
0rsMhJ0dwvG76XkjGhitbe82rQZqsnvLZ3qn8HHmtOFBLkQfGtT3K8nGOUuI42eF
H7HZKUCly2lCIizZdDVBkz4AWvaJlRc/3lE2Hd3Es6E52kTvROVKhdz06xuS8t5j
6twqKQKBgQC01AeiWL6Rzo+yZNzVgbpeeDogaZz5dtmURDgCYH8yFX5eoCKLHfnI
2nDIoqpaHY0LuX+dinuH+jP4tlyndbc2muXnHd9r0atytxA69ay3sSA5WFtfi4ef
ESElGO6qXEA821RpQp+2+uhL90+iC294cPqlS5LDmvTMypVDHzrxPQ==
-----END RSA PRIVATE KEY-----";

const SERVER_CERT: &str = "-----BEGIN CERTIFICATE-----
MIIDLjCCAhagAwIBAgIIW1mAtJWJAJYwDQYJKoZIhvcNAQELBQAwIDEeMBwGA1UE
AxMVbWluaWNhIHJvb3QgY2EgNGRjYzJmMCAXDTIzMDMwNjE2NDEwNloYDzIxMjMw
MzA2MTY0MTA2WjAUMRIwEAYDVQQDEwlsb2NhbGhvc3QwggEiMA0GCSqGSIb3DQEB
AQUAA4IBDwAwggEKAoIBAQCYMLJKooc+YRlKEMfeV09pX9myH34eUcUuT0fXS8lm
PlZ/NW7mm5lDwa8EUg61WuXQv2ouQDptmIcdeb/w4RW93Xflkyng1Xbd91OwQBJd
+8ZVBjzL7hSRk3QPDqx/CVBU/I1GmXKzb6cWzq1fTkOn1WLNXf21I6p7+N3qHLPF
JQeoVq1HBBFcAjTgJnpyQNvRGLDuLTK+OsWEGib2U8qrgiRdkaBLkxGXSlGABlOo
cQyW/zOhf4pwb2Z/JAge2mRW5IcexCPBWint8ydPsoJDds8j5+AyYCD6HUhHX0Ob
Qkz73OW7f2PQhuTK2uzKy0Yz6lNFt2nuzaWC04wIW3T7AgMBAAGjdjB0MA4GA1Ud
DwEB/wQEAwIFoDAdBgNVHSUEFjAUBggrBgEFBQcDAQYIKwYBBQUHAwIwDAYDVR0T
AQH/BAIwADAfBgNVHSMEGDAWgBTX46+p+Po1npE6QLQ7mMI+83s6qDAUBgNVHREE
DTALgglsb2NhbGhvc3QwDQYJKoZIhvcNAQELBQADggEBAAxrmQPG54ybKgMVliN8
Mg5povSdPIVVnlU/HOVG9yxzAOav/xQP003M4wqpatWxI8tR1PcLuZf0EPmcdJgb
tVl9nZMVZtveQnYMlU8PpkEVu56VM4Zr3rH9liPRlr0JEAXODdKw76kWKzmdqWZ/
rzhup3Ek7iEX6T5j/cPUvTWtMD4VEK2I7fgoKSHIX8MIVzqM7cuboGWPtS3eRNXl
MgvahA4TwLEXPEe+V1WAq6nSb4g2qSXWIDpIsy/O1WGS/zzRnKvXu9/9NkXWqZMl
C1LSpiiQUaRSglOvYf/Zx6r+4BOS4OaaArwHkecZQqBSCcBLEAyb/FaaXdBowI0U
PQ4=
-----END CERTIFICATE-----";
