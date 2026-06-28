use vector_sdk::VectorBot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Building bot...");
    let bot = VectorBot::builder()
        .public()
        .build()
        .await?;

    println!("Bot npub: {}", bot.npub());

    let core = bot.core();

    // List communities
    let communities = core.list_communities().await;
    println!("\nCommunities ({}):", communities.len());
    for c in &communities {
        println!("  {}", serde_json::to_string_pretty(c)?);
    }

    // List chats
    let chats = core.get_chats().await;
    println!("\nChats ({}):", chats.len());
    for c in &chats {
        println!("  {}", serde_json::to_string_pretty(c)?);
    }

    // Pending invites
    match core.list_pending_invites() {
        Ok(invites) => {
            println!("\nPending invites ({}):", invites.len());
            for i in &invites {
                println!("  {}", serde_json::to_string_pretty(i)?);
            }
        }
        Err(e) => println!("\nError listing pending invites: {:?}", e),
    }

    Ok(())
}
