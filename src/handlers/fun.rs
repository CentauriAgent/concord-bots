// =============================================================================
// handlers/fun.rs — Fun/gaming commands (8ball, flip, choose, rps)
// =============================================================================

use anyhow::Result;
use vector_sdk::IncomingMessage;

use crate::bot::BotContext;

// -----------------------------------------------------------------------------
// !8ball <question> — Magic 8-ball
// -----------------------------------------------------------------------------

const EIGHT_BALL_RESPONSES: &[&str] = &[
    // Positive
    "It is certain.",
    "It is decidedly so.",
    "Without a doubt.",
    "Yes definitely.",
    "You may rely on it.",
    "As I see it, yes.",
    "Most likely.",
    "Outlook good.",
    "Yes.",
    "Signs point to yes.",
    // Neutral
    "Reply hazy, try again.",
    "Ask again later.",
    "Better not tell you now.",
    "Cannot predict now.",
    "Concentrate and ask again.",
    // Negative
    "Don't count on it.",
    "My reply is no.",
    "My sources say no.",
    "Outlook not so good.",
    "Very doubtful.",
];

pub async fn eight_ball_command(
    ctx: &BotContext,
    msg: &IncomingMessage,
    args: &str,
) -> Result<()> {
    if args.trim().is_empty() {
        super::reply(ctx, msg, "🎱 Usage: !8ball <question>").await?;
        return Ok(());
    }

    use rand::seq::SliceRandom;
    let response = EIGHT_BALL_RESPONSES
        .choose(&mut rand::thread_rng())
        .unwrap_or(&"Maybe.");

    super::reply(ctx, msg, &format!("🎱 {}", response)).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !flip — Coin flip
// -----------------------------------------------------------------------------

pub async fn flip_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    use rand::Rng;
    let result = if rand::thread_rng().gen_bool(0.5) {
        "🪙 Heads!"
    } else {
        "🪙 Tails!"
    };
    super::reply(ctx, msg, result).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !choose <a | b | c> — Random choice
// -----------------------------------------------------------------------------

pub async fn choose_command(
    ctx: &BotContext,
    msg: &IncomingMessage,
    args: &str,
) -> Result<()> {
    let args = args.trim();

    if args.is_empty() {
        super::reply(ctx, msg, "Usage: !choose <option1 | option2 | option3>\nExample: !choose pizza | tacos | sushi").await?;
        return Ok(());
    }

    // Split on | and collect non-empty options.
    let options: Vec<&str> = args
        .split('|')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if options.len() < 2 {
        super::reply(ctx, msg, "⚠️ Give me at least 2 options separated by |").await?;
        return Ok(());
    }

    use rand::seq::SliceRandom;
    let choice = options
        .choose(&mut rand::thread_rng())
        .unwrap_or(&"???");

    super::reply(ctx, msg, &format!("🤔 I choose: **{}**", choice)).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !rps <rock|paper|scissors> — Rock Paper Scissors
// -----------------------------------------------------------------------------

pub async fn rps_command(
    ctx: &BotContext,
    msg: &IncomingMessage,
    args: &str,
) -> Result<()> {
    let player_choice = args.trim().to_lowercase();

    let player = match player_choice.as_str() {
        "rock" | "r" => "rock",
        "paper" | "p" => "paper",
        "scissors" | "s" => "scissors",
        _ => {
            super::reply(ctx, msg, "Usage: !rps <rock|paper|scissors>\nYou can also use: r, p, s").await?;
            return Ok(());
        }
    };

    use rand::seq::SliceRandom;
    let choices = ["rock", "paper", "scissors"];
    let bot_choice = choices.choose(&mut rand::thread_rng()).unwrap_or(&"rock");

    let result = determine_rps_winner(player, bot_choice);
    let emoji = |c: &str| match c {
        "rock" => "🪨",
        "paper" => "📄",
        "scissors" => "✂️",
        _ => "❓",
    };

    let response = match result {
        RpsResult::Win => format!(
            "{} vs {}\n🎉 You win!",
            emoji(player),
            emoji(bot_choice)
        ),
        RpsResult::Lose => format!(
            "{} vs {}\n💀 I win!",
            emoji(player),
            emoji(bot_choice)
        ),
        RpsResult::Tie => format!(
            "{} vs {}\n🤝 It's a tie!",
            emoji(player),
            emoji(bot_choice)
        ),
    };

    super::reply(ctx, msg, &response).await?;
    Ok(())
}

enum RpsResult {
    Win,
    Lose,
    Tie,
}

fn determine_rps_winner(player: &str, bot: &str) -> RpsResult {
    if player == bot {
        return RpsResult::Tie;
    }
    let player_wins = matches!(
        (player, bot),
        ("rock", "scissors") | ("paper", "rock") | ("scissors", "paper")
    );
    if player_wins {
        RpsResult::Win
    } else {
        RpsResult::Lose
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rps_winner() {
        assert!(matches!(determine_rps_winner("rock", "scissors"), RpsResult::Win));
        assert!(matches!(determine_rps_winner("paper", "rock"), RpsResult::Win));
        assert!(matches!(determine_rps_winner("scissors", "paper"), RpsResult::Win));

        assert!(matches!(determine_rps_winner("rock", "paper"), RpsResult::Lose));
        assert!(matches!(determine_rps_winner("paper", "scissors"), RpsResult::Lose));
        assert!(matches!(determine_rps_winner("scissors", "rock"), RpsResult::Lose));

        assert!(matches!(determine_rps_winner("rock", "rock"), RpsResult::Tie));
        assert!(matches!(determine_rps_winner("paper", "paper"), RpsResult::Tie));
        assert!(matches!(determine_rps_winner("scissors", "scissors"), RpsResult::Tie));
    }

    #[test]
    fn test_eight_ball_responses_exist() {
        assert!(EIGHT_BALL_RESPONSES.len() == 20);
        // Classic 8-ball has 20 answers
    }
}
