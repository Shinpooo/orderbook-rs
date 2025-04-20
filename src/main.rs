use ordered_float::OrderedFloat;
use std::{
    cell::RefCell,
    collections::{btree_map::Entry, BTreeMap, HashMap, VecDeque},
    rc::Rc,
};

use futures_util::{future, StreamExt, TryStreamExt};
use tokio::net::{TcpListener, TcpStream};

use serde::Deserialize;
use tokio_tungstenite::tungstenite::protocol::Message;
macro_rules! log_match {
    ($side:expr, $taker_id:expr, $maker_id:expr, $price:expr, $amount:expr) => {
        println!(
            "🔁 MATCH {:?} | Taker ID: {}, Maker ID: {} | Price: {:.2}, Amount: {:.2}",
            $side, $taker_id, $maker_id, $price, $amount
        );
    };
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
enum Side {
    BUY,
    SELL,
}

#[derive(Clone, Debug)]
struct Order {
    id: u32,
    price: f64,
    amount: f64,
    side: Side,
}

impl Order {
    fn new(id: u32, price: f64, amount: f64, side: Side) -> Self {
        Self {
            id,
            price,
            amount,
            side,
        }
    }
}

type Price = OrderedFloat<f64>;
type OrderId = u32;

type OrderNodeRef = Rc<RefCell<OrderNode>>;
type OrderMapEntry = (Side, Price, OrderNodeRef);

// Order book level is structure as a double pointer , this way we can pass a direct

struct OrderNode {
    order: Order,
    previous: Option<OrderNodeRef>,
    next: Option<OrderNodeRef>,
}

struct OrderBookSide {
    levels: BTreeMap<Price, (Option<OrderNodeRef>, Option<OrderNodeRef>)>,
    best_price: Option<Price>,
}

impl OrderBookSide {
    fn new() -> Self {
        Self {
            levels: BTreeMap::new(),
            best_price: None,
        }
    }
    fn print_level(&self, price: f64) {
        let price = OrderedFloat(price);
        match self.levels.get(&price) {
            Some((Some(head), _)) => {
                println!("📈 Price Level {:.4}:", price);

                let mut current = Some(Rc::clone(head));
                while let Some(node) = current {
                    let order = &node.borrow().order;
                    println!(
                        "    🧾 Order ID: {}, Amount: {:.2}, Side: {:?}",
                        order.id, order.amount, order.side
                    );
                    current = node.borrow().next.clone();
                }
            }
            Some((None, _)) => {
                println!("⚠️ Price level {:.4} exists but is empty.", price);
            }
            None => {
                println!("❌ Price level {:.4} not found.", price);
            }
        }
    }

    fn insert_node(&mut self, price: Price, node_ref: OrderNodeRef, side: Side) {
        match self.levels.entry(price) {
            Entry::Vacant(entry) => {
                // Price level doesn't exist; insert new head and tail
                entry.insert((Some(Rc::clone(&node_ref)), Some(Rc::clone(&node_ref))));
                match self.best_price {
                    None => self.best_price = Some(price),
                    Some(best) => {
                        let should_update = match side {
                            Side::BUY => price > best,
                            Side::SELL => price < best,
                        };
                        if should_update {
                            self.best_price = Some(price)
                        }
                    }
                }
            }
            Entry::Occupied(mut entry) => {
                let (head, tail) = entry.get_mut();
                if let Some(tail_node) = tail {
                    // Append to the existing tail
                    tail_node.borrow_mut().next = Some(Rc::clone(&node_ref));
                    node_ref.borrow_mut().previous = Some(Rc::clone(tail_node));
                    *tail = Some(Rc::clone(&node_ref));
                } else {
                    // Edge case: tail is None; reset head and tail
                    *head = Some(Rc::clone(&node_ref));
                    *tail = Some(Rc::clone(&node_ref));
                }
            }
        }
    }

    fn remove_node(&mut self, price: Price, node_ref: &OrderNodeRef, side: Side) {
        if let Some((head_opt, tail_opt)) = self.levels.get_mut(&price) {
            let prev = node_ref.borrow().previous.clone();
            let next = node_ref.borrow().next.clone();

            // Re-link neighbors
            if let Some(prev_node) = &prev {
                prev_node.borrow_mut().next = next.clone();
            } else {
                // This node was head
                *head_opt = next.clone();
            }

            if let Some(next_node) = &next {
                next_node.borrow_mut().previous = prev.clone();
            } else {
                // This node was tail
                *tail_opt = prev.clone();
            }

            // In case there is no more order at the current level
            // If head and tail are now None, delete the whole level
            // Also adjust the best price
            if head_opt.is_none() && tail_opt.is_none() {
                self.levels.remove(&price);
                if Some(price) == self.best_price {
                    self.best_price = match side {
                        Side::BUY => self.levels.keys().next().cloned(),
                        Side::SELL => self.levels.keys().next().cloned(),
                    }
                }
            }
        }
    }
}

struct Orderbook {
    order_map: HashMap<OrderId, OrderMapEntry>,
    bids: OrderBookSide,
    asks: OrderBookSide,
}

impl Orderbook {
    fn new() -> Self {
        Self {
            order_map: HashMap::new(),
            bids: OrderBookSide::new(),
            asks: OrderBookSide::new(),
        }
    }

    fn insert_order(&mut self, order: Order) {
        let mut remaining = order.amount;
        let price = OrderedFloat(order.price);

        match order.side {
            Side::BUY => {
                while let Some(best_ask) = self.asks.best_price {
                    if price < best_ask || remaining <= 0.0 {
                        break;
                    }

                    if let Some((Some(head_ref), _)) = self.asks.levels.get_mut(&best_ask) {
                        let mut current = Some(Rc::clone(head_ref));
                        while let Some(node) = current {
                            let matched;
                            let next;
                            let id_to_remove;
                            let mut should_remove = false;

                            {
                                let mut node_borrow = node.borrow_mut();
                                matched = remaining.min(node_borrow.order.amount);

                                log_match!(
                                    Side::BUY,
                                    order.id,
                                    node_borrow.order.id,
                                    best_ask,
                                    matched
                                );

                                node_borrow.order.amount -= matched;
                                remaining -= matched;

                                next = node_borrow.next.clone();
                                id_to_remove = node_borrow.order.id;

                                if node_borrow.order.amount <= 0.0 {
                                    should_remove = true;
                                }
                            } // borrow_mut ends here ✅

                            if should_remove {
                                self.asks.remove_node(best_ask, &node, Side::SELL);
                                self.order_map.remove(&id_to_remove);
                            }

                            if remaining <= 0.0 {
                                break;
                            }

                            current = next;
                        }
                    } else {
                        break;
                    }
                }

                if remaining > 0.0 {
                    let node_ref = Rc::new(RefCell::new(OrderNode {
                        order: Order::new(order.id, order.price, remaining, Side::BUY),
                        previous: None,
                        next: None,
                    }));
                    self.bids
                        .insert_node(price, Rc::clone(&node_ref), Side::BUY);
                    self.order_map
                        .insert(order.id, (Side::BUY, price, node_ref));
                }
            }

            Side::SELL => {
                while let Some(best_bid) = self.bids.best_price {
                    if price > best_bid || remaining <= 0.0 {
                        break;
                    }

                    if let Some((Some(head_ref), _)) = self.bids.levels.get_mut(&best_bid) {
                        let mut current = Some(Rc::clone(head_ref));
                        while let Some(node) = current {
                            let matched;
                            let next;
                            let id_to_remove;
                            let mut should_remove = false;

                            {
                                let mut node_borrow = node.borrow_mut();
                                matched = remaining.min(node_borrow.order.amount);

                                log_match!(
                                    Side::SELL,
                                    order.id,
                                    node_borrow.order.id,
                                    best_bid,
                                    matched
                                );

                                node_borrow.order.amount -= matched;
                                remaining -= matched;

                                next = node_borrow.next.clone();
                                id_to_remove = node_borrow.order.id;

                                if node_borrow.order.amount <= 0.0 {
                                    should_remove = true;
                                }
                            }

                            if should_remove {
                                self.bids.remove_node(best_bid, &node, Side::BUY);
                                self.order_map.remove(&id_to_remove);
                            }

                            if remaining <= 0.0 {
                                break;
                            }

                            current = next;
                        }
                    } else {
                        break;
                    }
                }

                if remaining > 0.0 {
                    let node_ref = Rc::new(RefCell::new(OrderNode {
                        order: Order::new(order.id, order.price, remaining, Side::SELL),
                        previous: None,
                        next: None,
                    }));
                    self.asks
                        .insert_node(price, Rc::clone(&node_ref), Side::SELL);
                    self.order_map
                        .insert(order.id, (Side::SELL, price, node_ref));
                }
            }
        }
    }
    fn cancel_order(&mut self, order_id: u32) {
        if let Some((side, price, node_ref)) = self.order_map.remove(&order_id) {
            match side {
                Side::BUY => self.bids.remove_node(price, &node_ref, side),
                Side::SELL => self.asks.remove_node(price, &node_ref, side),
            }
            println!("❌ Cancelled order ID {}", order_id);
        } else {
            println!("⚠️ Order ID {} not found", order_id);
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClientMessage {
    #[serde(rename = "create_order")]
    CreateOrder { price: f64, amount: f64, side: Side },
}

async fn accept_connection(stream: TcpStream) {
    let peer_address = stream.peer_addr().expect("Peer should have an address.");
    println!("Peer address: {}", peer_address);

    let ws_stream = tokio_tungstenite::accept_async(stream)
        .await
        .expect("Error during the websocket handshake");

    println!("New websocket connection established.");

    let (write, mut read) = ws_stream.split();

    while let Some(msg_result) = read.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                // Try to parse as our ClientMessage
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::CreateOrder {
                        price,
                        amount,
                        side,
                    }) => {
                        println!(
                            "📦 create_order → price={}, amount={}, side={:?}",
                            price, amount, side
                        );
                        // → call your orderbook.insert_order(...)
                    }
                    // Ok(ClientMessage::CancelOrder { id }) => {
                    //     println!("❌ cancel_order → id={}", id);
                    //     // → call your orderbook.cancel_order(id)
                    // }
                    Err(e) => {
                        eprintln!("⚠️ malformed JSON message: {}\n  raw: {}", e, text);
                        // optionally send an error back to client
                    }
                }
            }

            Ok(Message::Close(frame)) => {
                println!("👋 Client closed connection: {:?}", frame);
                break;
            }

            Ok(other) => {
                println!("🔷 ignoring non‐text WebSocket message: {:?}", other);
            }

            Err(e) => {
                eprintln!("❌ WebSocket error: {}", e);
                break;
            }
        }
    }

    println!("🛑 Connection handler exiting");
}

#[tokio::main]
async fn main() {
    let address = "127.0.0.1:8000";
    let listener: Result<TcpListener, std::io::Error> = TcpListener::bind(&address).await;
    let listener = match listener {
        Ok(listener) => {
            println!("TCP Listener binded to {}", address);
            listener
        }
        Err(e) => {
            println!("Failed to bind to {}, Error: {}", address, e);
            return;
        }
    };

    loop {
        let stream: tokio::net::TcpStream = match listener.accept().await {
            Ok((tcp_stream, _)) => tcp_stream,
            Err(e) => {
                println!("An error happened {}", e);
                return;
            }
        };
        tokio::spawn(accept_connection(stream));
    }

    // let mut orderbook: Orderbook = Orderbook::new();
    // let new_order: Order = Order::new(1, 5.5, 30.0, Side::BUY);
    // orderbook.insert_order(new_order);
    // orderbook.bids.print_level(5.5);
    // orderbook.asks.print_level(30.0);
    // let new_order: Order = Order::new(2, 5.5, 30.0, Side::BUY);
    // orderbook.insert_order(new_order);
    // orderbook.bids.print_level(5.5);
    // orderbook.cancel_order(1);
    // orderbook.bids.print_level(5.5);
    // orderbook.cancel_order(2);
    // orderbook.bids.print_level(5.5);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_book() -> Orderbook {
        Orderbook::new()
    }

    #[test]
    fn test_insert_limit_order_no_match() {
        let mut ob = setup_book();
        ob.insert_order(Order::new(1, 10.0, 5.0, Side::BUY));
        assert!(ob.bids.levels.contains_key(&OrderedFloat(10.0)));
        assert_eq!(ob.order_map.len(), 1);
    }

    #[test]
    fn test_simple_match_buy_consumes_sell() {
        let mut ob = setup_book();

        // Insert a sell order first
        ob.insert_order(Order::new(1, 9.5, 5.0, Side::SELL));

        // Insert a buy order that crosses
        ob.insert_order(Order::new(2, 10.0, 5.0, Side::BUY));

        // Order book should be empty now
        assert!(ob.bids.levels.is_empty());
        assert!(ob.asks.levels.is_empty());
        assert!(ob.order_map.is_empty());
    }

    #[test]
    fn test_partial_match() {
        let mut ob = setup_book();

        // Insert a big sell order
        ob.insert_order(Order::new(1, 10.0, 10.0, Side::SELL));

        // Insert a smaller buy order
        ob.insert_order(Order::new(2, 10.0, 4.0, Side::BUY));

        // The sell order should be partially filled (6.0 left)
        let remaining = ob
            .asks
            .levels
            .get(&OrderedFloat(10.0))
            .unwrap()
            .0
            .as_ref()
            .unwrap()
            .borrow()
            .order
            .amount;

        assert_eq!(remaining, 6.0);
    }

    #[test]
    fn test_cancel_order() {
        let mut ob = setup_book();

        ob.insert_order(Order::new(1, 12.0, 3.0, Side::SELL));
        ob.cancel_order(1);

        assert!(!ob.order_map.contains_key(&1));
        assert!(ob.asks.levels.get(&OrderedFloat(12.0)).is_none());
    }

    #[test]
    fn test_best_price_tracking() {
        let mut ob = setup_book();
        ob.insert_order(Order::new(1, 9.0, 1.0, Side::SELL));
        ob.insert_order(Order::new(2, 8.0, 1.0, Side::SELL));

        // Should track best ask as 8.0
        assert_eq!(ob.asks.best_price, Some(OrderedFloat(8.0)));

        ob.cancel_order(2); // remove best price

        assert_eq!(ob.asks.best_price, Some(OrderedFloat(9.0)));
    }

    #[test]
    fn test_multi_level_match_buy_sweeps_sell_levels() {
        let mut ob = setup_book();

        // Insert multiple sell orders at different levels
        ob.insert_order(Order::new(1, 9.0, 2.0, Side::SELL));
        ob.insert_order(Order::new(2, 9.5, 3.0, Side::SELL));
        ob.insert_order(Order::new(3, 10.0, 5.0, Side::SELL));

        // Buy order with enough amount to sweep all levels (10.0 units @ 11.0)
        ob.insert_order(Order::new(10, 11.0, 10.0, Side::BUY));

        // All asks should be gone
        assert!(ob.asks.levels.is_empty());
        assert!(ob.order_map.is_empty());
    }

    #[test]
    fn test_multi_level_match_partial_sweep() {
        let mut ob = setup_book();

        ob.insert_order(Order::new(1, 9.0, 2.0, Side::SELL));
        ob.insert_order(Order::new(2, 9.5, 3.0, Side::SELL));
        ob.insert_order(Order::new(3, 10.0, 5.0, Side::SELL));

        // Taker can only buy 6.0 — will sweep 9.0 and 9.5, partially consume 10.0
        ob.insert_order(Order::new(11, 11.0, 6.0, Side::BUY));

        // Should leave 4.0 at 10.0
        let remaining = ob
            .asks
            .levels
            .get(&OrderedFloat(10.0))
            .unwrap()
            .0
            .as_ref()
            .unwrap()
            .borrow()
            .order
            .amount;

        assert_eq!(remaining, 4.0);
        assert_eq!(ob.asks.levels.len(), 1);
    }

    #[test]
    fn test_match_leaves_book_state_correct() {
        let mut ob = setup_book();

        // 2 sellers at the same level
        ob.insert_order(Order::new(1, 10.0, 4.0, Side::SELL));
        ob.insert_order(Order::new(2, 10.0, 6.0, Side::SELL));

        // Buyer takes 7.0 — should fully fill order 1, partially fill order 2
        ob.insert_order(Order::new(3, 11.0, 7.0, Side::BUY));

        // Order 2 should remain with 3.0 at 10.0
        let remaining = ob
            .asks
            .levels
            .get(&OrderedFloat(10.0))
            .unwrap()
            .0
            .as_ref()
            .unwrap()
            .borrow()
            .order
            .amount;

        assert_eq!(remaining, 3.0);
        assert!(ob.order_map.contains_key(&2));
        assert!(!ob.order_map.contains_key(&1));
    }
}
