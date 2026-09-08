# AI Bridge

Turn a Concord bot into a community AI bridge. Members ask questions, summarize
long pastes, check sentiment, or generate images — all inside the chat.

## Enable

`config/bot.toml`:

```toml
[features]
ai = true

[ai]
provider = "openai"        # OpenAI-compatible chat completions
model = "gpt-4o-mini"      # any model your endpoint serves
# api_key = "***"       # or set the AI_API_KEY env var
# system_prompt = "You are a helpful assistant for a Nostr community."
```

Any OpenAI-wire-compatible endpoint works: OpenAI, OpenRouter, or self-hosted
llama.cpp / vLLM gateways.

## Usage

```text
!ask what's the difference between NIP-17 and NIP-04 DMs?
!summarize <paste of a long post>
!sentiment this community is on fire today 🚀
!image a lighthouse beaming purple light over a calm sea
```

Smoke test after enabling: `!ask give me a one-line haiku about nostr`.

## Notes

- `!image` uses the OpenAI Images API — confirm your endpoint serves it.
- Keep `api_key` in config (chmod 600) or env; never commit either.
- AI commands are Public: any community member can spend your tokens — budget accordingly or restrict via your provider's rate limits.
