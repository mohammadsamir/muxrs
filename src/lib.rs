mod frame;
mod header;
mod session;
mod stream;

#[cfg(test)]
mod tests {
    use crate::session::{MuxMode, MuxSession};
    use futures::AsyncWriteExt;
    use log::info;
    use std::time::Duration;
    use tokio::{
        net::{TcpListener, TcpStream},
        time::sleep,
    };
    use tokio_util::compat::TokioAsyncReadCompatExt;

    #[tokio::test]
    async fn server() {
        env_logger::init();

        let listener = TcpListener::bind("127.0.0.1:5000").await.unwrap();
        loop {
            let (socket, _) = listener.accept().await.unwrap();

            let session = MuxSession::new(socket.compat()).await;
            let cloned = session.clone();
            let mut stream = session.open(MuxMode::Server).await;
            tokio::spawn(async move {
                loop {
                    stream
                        .write("ping".as_bytes())
                        .await
                        .expect("failed to write to stream");
                    info!("frame sent to stream {}", stream.id());
                    sleep(Duration::from_millis(1000)).await;
                }
            });

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
            cloned.run().await
        }
    }

    #[tokio::test]
    async fn client() {
        env_logger::init();

        let socket = TcpStream::connect("127.0.0.1:5000").await.unwrap();
        let session = MuxSession::new(socket.compat()).await;
        let cloned = session.clone();
        let mut stream_a = session.open(MuxMode::Client).await;
        let mut stream_b = session.open(MuxMode::Client).await;

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
}
