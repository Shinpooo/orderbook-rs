use futures_util::SinkExt;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

use std::thread;
use std::time::Duration;

use serde_json::json;
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
            // build your JSON string
            let msg = json!({
                "type": "create_order",
                "id": 1,
                "price": 5.5,
                "amount": 2.0,
                "side": "BUY"
            })
            .to_string();

            // Send as binary UTF‑8 bytes instead of a Text frame

            ws_stream.send(Message::text(msg)).await.unwrap();

            ws_stream.send(Message::Close(None)).await.unwrap();
        }
        Err(e) => println!("Error while connecting"),
    }
}
