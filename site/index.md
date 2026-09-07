---
layout: home

hero:
  name: concord-bots
  text: Vector/Concord Protocol bot template
  tagline: Build custom Nostr bots by writing simple handler functions — no Rust expertise required. The framework handles the connection, encryption, reconnection, message routing, and scheduling.
  image:
    src: /favicon.svg
    alt: concord-bots
  actions:
    - theme: brand
      text: Get Started
      link: /guide/getting-started
    - theme: alt
      text: Command Reference
      link: /guide/commands
    - theme: alt
      text: View on GitHub
      link: https://github.com/CentauriAgent/concord-bots

features:
  - icon: 🤖
    title: Agent-first template
    details: Drop AGENTS.md in front of any AI agent (OpenClaw, Claude, Cursor...) and it knows exactly where handlers go, what's safe to edit, and how to deploy.
  - icon: ⚡
    title: Concord v2 communities
    details: Create and manage v2 communities, permission-aware moderation with automatic rekeys on bans, message delete/edit, and file handling — while staying compatible with v1.
  - icon: 🛡️
    title: Built-in auth & moderation
    details: Three permission levels (Public / Authorized / Owner), persistent auth state, and a moderation suite with kick, ban, warn, and warnings history.
  - icon: 💜
    title: Cashu wallet & zaps
    details: Built-in ecash wallet — send and receive sats, tip in chat, and full NIP-57 zap flow with automatic npub.cash claims every 5 minutes.
  - icon: 🧠
    title: Optional AI bridge
    details: "!ask, !summarize, !sentiment and !image commands, plus optional AI replies to non-command messages via OpenClaw or any OpenAI API key."
  - icon: 🚀
    title: Deploy anywhere
    details: One-command systemd install or multi-stage Docker image — and this very documentation site is published to Nostr with nsite, no GitHub Pages required.
---
