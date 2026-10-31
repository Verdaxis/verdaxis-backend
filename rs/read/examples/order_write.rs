//! Disposable Task 20 adapter; never part of the service binary.
#[cfg(feature = "contract-tests")]
#[tokio::main]
async fn main() {
    verdaxis_read::order_write::serve_contract().await;
}

#[cfg(not(feature = "contract-tests"))]
fn main() {}
