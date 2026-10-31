#[cfg(feature = "contract-tests")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    verdaxis_read::crud::alerts::contract::serve().await
}
#[cfg(not(feature = "contract-tests"))]
fn main() {
    panic!("contract-tests feature required");
}
