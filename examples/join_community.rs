use vector_sdk::VectorBot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let invite_url = std::env::args()
        .nth(1)
        .expect("Usage: cargo run --example join_community -- <invite_url>");

    println!("Building bot from existing identity...");
    let bot = VectorBot::builder()
        .public()
        .build()
        .await?;

    println!("Bot online as {}", bot.npub());
    println!("Joining community from invite URL...");
    println!("URL: {}", invite_url);

    let result = bot.core().join_community(&invite_url).await?;

    println!("\n✅ Joined! Result:");
    println!("{}", serde_json::to_string_pretty(&result)?);

    Ok(())
}
