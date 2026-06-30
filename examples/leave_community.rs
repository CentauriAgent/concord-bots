use vector_sdk::VectorBot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let community_id = std::env::args()
        .nth(1)
        .expect("Usage: cargo run --example leave_community -- <community_id>");

    println!("Building bot from existing identity...");
    let bot = VectorBot::builder()
        .public()
        .build()
        .await?;

    println!("Bot online as {}", bot.npub());
    println!("Leaving community: {}", community_id);

    match bot.core().leave_community(&community_id).await {
        Ok(()) => {
            println!("✅ Successfully left community {}", community_id);
            println!("   Leave presence event published to relays.");
        }
        Err(e) => {
            eprintln!("❌ Failed to leave community: {:?}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}
