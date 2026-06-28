use std::path::PathBuf;
use vector_sdk::VectorBot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let avatar_path = PathBuf::from("/tmp/flagship-avatar.png");
    let banner_path = PathBuf::from("/tmp/flagship-banner.png");

    println!("Building bot...");
    let bot = VectorBot::builder().public().build().await?;
    println!("Bot online as {}", bot.npub());

    println!("Uploading avatar...");
    let avatar_url = bot.upload_image(&avatar_path).await?;
    println!("Avatar URL: {}", avatar_url);

    println!("Uploading banner...");
    let banner_url = bot.upload_image(&banner_path).await?;
    println!("Banner URL: {}", banner_url);

    println!("Updating profile to Flagship...");
    let success = bot.update_profile(
        "Flagship",
        &avatar_url,
        &banner_url,
        "🚢 Lead vessel of the Concord fleet. Setting the course for all who follow.",
    ).await;

    if success {
        println!("✅ Profile updated! Avatar: {} Banner: {}", avatar_url, banner_url);
    } else {
        println!("❌ Profile update failed");
    }

    Ok(())
}
