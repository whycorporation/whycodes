//! Attach to a running `whycode serve` and list sessions.
//!
//! ```bash
//! whycode serve
//! cargo run -p whycode-sdk --example connect
//! ```

use whycode_sdk::WhycodeClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:3030".into());
    let client = WhycodeClient::connect(&addr).await?;
    let health = client.health().await?;
    println!(
        "protocol={} version={} project={}",
        health.protocol, health.version, health.project
    );
    for s in client.list_sessions().await? {
        println!("  {}  {}", s.id, s.title);
    }
    Ok(())
}
