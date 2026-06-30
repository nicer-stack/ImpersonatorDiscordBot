# Discord Trigger Webhook Bot

Installation:
Add it to your server by clicking [Here](https://discord.com/oauth2/authorize?client_id=1519985903089487952)



A Serenity Discord bot written in Rust. Users can create text triggers, and when a later message contains a trigger, the bot relays the saved response through a webhook using the original trigger creator's display name and avatar.

## Features

- `!trigger add <trigger> | <message>` saves a trigger for the current server.
- `!trigger remove <trigger>` removes one of your own triggers.
- `!trigger list` shows saved triggers in the server.
- Trigger matches are case-insensitive and work when the trigger appears anywhere in a message.
- Webhook messages use the trigger creator's username and avatar URL.
- Triggers are persisted in `triggers.json`.

## Discord Setup

1. Create an application and bot in the Discord Developer Portal.
2. Enable the `MESSAGE CONTENT INTENT` for the bot.
3. Invite the bot with these permissions:
   - Read Messages/View Channels
   - Send Messages
   - Manage Webhooks
4. Put the bot token in your environment:

```powershell
$env:DISCORD_TOKEN="your-token-here"
```

Optional settings:

```powershell
$env:TRIGGER_PREFIX="!trigger"
$env:TRIGGER_STORAGE="triggers.json"
```

## Run

```powershell
cargo run
```

## Example

```text
!trigger add hello bot | Hello from the saved trigger!
```

After that, if anyone says:

```text
well hello bot
```

the bot creates or reuses a webhook named `Trigger Relay` in that channel and sends:

```text
Hello from the saved trigger!
```

The webhook message is posted with the name and avatar of the user who created the trigger.

## Notes

- Webhooks cannot be created in DMs, so triggers only work in servers.
- Discord webhooks allow overriding username and avatar per message, so the bot keeps one channel webhook and sends each trigger response with the saved creator identity.
- If webhook creation fails, check that the bot has `Manage Webhooks` in the channel.
- Large Language Models were used in the process of creating this.
