use futures_util::SinkExt;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

use std::thread;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let server_address = "ws://127.0.0.1:8000";
    let connecton = connect_async(server_address).await;
    match connecton {
        Ok((mut ws_stream, _)) => {
            println!("Connected to server");
            // Send something
            // ws_stream
            //     .send(Message::Text("Hello Server!"))
            //     .await
            //     .unwrap();
            //
            // Gracefully close the socket
            // thread::sleep(Duration::from_secs(3));

            ws_stream.send(Message::Close(None)).await.unwrap();
        }
        Err(e) => println!("Error while connecting"),
    }
}
