# Muxrs (WIP)

An experimental, bidirectional multiplexing library written in Rust

### Usage
```rust
// server
async fn server() {
    let listener = TcpListener::bind("127.0.0.1:5000").await.unwrap();
    // keep accepting incoming connections
    loop {
        let (socket, _) = listener.accept().await.unwrap();

        // create a MuxSession from the connection
        let session = MuxSession::new(socket.compat()).await;
        let cloned = session.clone();

        // keep accepting incoming streams
        tokio::spawn(async move {
            loop {
                let mut stream = session.accept().await;
                tokio::spawn(async move {
                    loop {
                        let frame = stream.read_frame().await;
                        info!("frame received from stream {}: {:?}", stream.id(), frame);
                    }
                });
            }
        });

        // start the session
        cloned.run().await
    }
}

// client
async fn client() {

    let socket = TcpStream::connect("127.0.0.1:5000").await.unwrap();

    // create a MuxSession
    let session = MuxSession::new(socket.compat()).await;
    let cloned = session.clone();

    // create two streams
    let mut stream_a = session.open(MuxMode::Client).await;
    let mut stream_b = session.open(MuxMode::Client).await;

    // write to streams concurrently
    tokio::spawn(async move {
        loop {
            stream_a
                .write("ping".as_bytes())
                .await
                .expect("failed to write to stream");
            info!("frame sent to stream {}", stream_a.id());
            sleep(Duration::from_millis(1000)).await;
        }
    });

    tokio::spawn(async move {
        loop {
            stream_b
                .write("ping".as_bytes())
                .await
                .expect("failed to write to stream");
            info!("frame sent to stream {}", stream_b.id());
            sleep(Duration::from_millis(1000)).await;
        }
    });

    // listen for and read from incoming streams
    tokio::spawn(async move {
        loop {
            let mut stream = session.accept().await;
            tokio::spawn(async move {
                loop {
                    let frame = stream.read_frame().await;
                    info!("frame received from stream {}: {:?}", stream.id(), frame);
                }
            });
        }
    });
    cloned.run().await;
}
```

### Tests

```bash
# run the server
RUST_LOG=info cargo test server -- --nocapture

# run the client
RUST_LOG=info cargo test client -- --nocapture
```
