use std::{collections::HashMap, env, path::PathBuf, sync::Arc};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serenity::{
    async_trait,
    builder::{CreateWebhook, ExecuteWebhook},
    model::{channel::Message, gateway::Ready, webhook::Webhook},
    prelude::*,
};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

const WEBHOOK_NAME: &str = "Trigger Relay";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Trigger {
    trigger: String,
    response: String,
    creator_id: u64,
    creator_name: String,
    creator_avatar_url: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct StoredTriggers {
    servers: HashMap<String, Vec<Trigger>>,
}

struct BotState {
    prefix: String,
    storage_path: PathBuf,
    triggers: RwLock<StoredTriggers>,
}

struct StateKey;

impl TypeMapKey for StateKey {
    type Value = Arc<BotState>;
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        info!("Logged in as {}", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot || msg.webhook_id.is_some() {
            return;
        }

        let state = {
            let data = ctx.data.read().await;
            data.get::<StateKey>().cloned()
        };

        let Some(state) = state else {
            error!("Bot state was not registered");
            return;
        };

        if msg.content.starts_with(&state.prefix) {
            if let Err(err) = handle_command(&ctx, &msg, &state).await {
                warn!("Command failed: {err:?}");
                let _ = msg
                    .channel_id
                    .say(&ctx.http, format!("Could not do that: {err}"))
                    .await;
            }
            return;
        }

        if let Err(err) = maybe_fire_trigger(&ctx, &msg, &state).await {
            warn!("Trigger handling failed: {err:?}");
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "discord_trigger_webhook_bot=info,serenity=warn".into()),
        )
        .init();

    let token = env::var("DISCORD_TOKEN").context("Set DISCORD_TOKEN to your bot token")?;
    let prefix = env::var("TRIGGER_PREFIX").unwrap_or_else(|_| "!trigger".to_string());
    let storage_path = env::var("TRIGGER_STORAGE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("triggers.json"));

    let triggers = load_triggers(&storage_path).await?;
    let state = Arc::new(BotState {
        prefix,
        storage_path,
        triggers: RwLock::new(triggers),
    });

    let intents =
        GatewayIntents::GUILDS | GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

    let mut client = Client::builder(token, intents)
        .event_handler(Handler)
        .await
        .context("Failed to create Discord client")?;

    {
        let mut data = client.data.write().await;
        data.insert::<StateKey>(state);
    }

    client.start().await.context("Discord client stopped")?;
    Ok(())
}

async fn handle_command(ctx: &Context, msg: &Message, state: &Arc<BotState>) -> Result<()> {
    let command = msg.content[state.prefix.len()..].trim();

    if command == "list" {
        return list_triggers(ctx, msg, state).await;
    }

    let Some(split_at) = command.find(char::is_whitespace) else {
        send_help(ctx, msg, state).await?;
        return Ok(());
    };

    let (name, rest) = command.split_at(split_at);
    let rest = rest.trim();

    match name {
        "add" => add_trigger(ctx, msg, state, rest).await,
        "remove" | "delete" => remove_trigger(ctx, msg, state, rest).await,
        _ => send_help(ctx, msg, state).await,
    }
}

async fn send_help(ctx: &Context, msg: &Message, state: &Arc<BotState>) -> Result<()> {
    let help = format!(
        "Commands:\n`{0} add <trigger> | <message>`
`{0} remove <trigger>`
`{0} list`",
        state.prefix
    );
    msg.channel_id.say(&ctx.http, help).await?;
    Ok(())
}

async fn add_trigger(
    ctx: &Context,
    msg: &Message,
    state: &Arc<BotState>,
    args: &str,
) -> Result<()> {
    let guild_id = msg
        .guild_id
        .context("Triggers can only be created inside a server")?;
    let (trigger_text, response) = args
        .split_once('|')
        .context("Use this format: `!trigger add <trigger> | <message>`")?;

    let trigger_text = trigger_text.trim();
    let response = response.trim();

    if trigger_text.is_empty() || response.is_empty() {
        anyhow::bail!("The trigger and message both need text");
    }

    let trigger = Trigger {
        trigger: trigger_text.to_string(),
        response: response.to_string(),
        creator_id: msg.author.id.get(),
        creator_name: display_name(msg),
        creator_avatar_url: msg
            .author
            .avatar_url()
            .unwrap_or_else(|| msg.author.default_avatar_url()),
    };

    {
        let mut stored = state.triggers.write().await;
        let server_triggers = stored.servers.entry(guild_id.get().to_string()).or_default();

        if let Some(existing) = server_triggers.iter_mut().find(|item| {
            item.trigger.eq_ignore_ascii_case(trigger_text) && item.creator_id == msg.author.id.get()
        }) {
            *existing = trigger;
        } else {
            server_triggers.push(trigger);
        }

        save_triggers(&state.storage_path, &stored).await?;
    }

    msg.channel_id
        .say(
            &ctx.http,
            format!("Saved trigger `{trigger_text}`. I will relay it as {}.", display_name(msg)),
        )
        .await?;

    Ok(())
}

async fn remove_trigger(
    ctx: &Context,
    msg: &Message,
    state: &Arc<BotState>,
    trigger_text: &str,
) -> Result<()> {
    let guild_id = msg
        .guild_id
        .context("Triggers can only be removed inside a server")?;

    if trigger_text.is_empty() {
        anyhow::bail!("Tell me which trigger to remove");
    }

    let removed = {
        let mut stored = state.triggers.write().await;
        let server_triggers = stored.servers.entry(guild_id.get().to_string()).or_default();
        let before = server_triggers.len();

        server_triggers.retain(|item| {
            !(item.trigger.eq_ignore_ascii_case(trigger_text)
                && item.creator_id == msg.author.id.get())
        });

        let removed = before - server_triggers.len();
        save_triggers(&state.storage_path, &stored).await?;
        removed
    };

    if removed == 0 {
        msg.channel_id
            .say(
                &ctx.http,
                "I did not find one of your triggers with that text.",
            )
            .await?;
    } else {
        msg.channel_id.say(&ctx.http, "Removed trigger.").await?;
    }

    Ok(())
}

async fn list_triggers(ctx: &Context, msg: &Message, state: &Arc<BotState>) -> Result<()> {
    let guild_id = msg
        .guild_id
        .context("Triggers can only be listed inside a server")?;

    let lines = {
        let stored = state.triggers.read().await;
        stored
            .servers
            .get(&guild_id.get().to_string())
            .cloned()
            .unwrap_or_default()
    };

    if lines.is_empty() {
        msg.channel_id.say(&ctx.http, "No triggers saved yet.").await?;
        return Ok(());
    }

    let mut reply = String::from("Saved triggers:
");
    for trigger in lines.iter().take(25) {
        reply.push_str(&format!(
            "- `{}` by {}
",
            trigger.trigger, trigger.creator_name
        ));
    }

    if lines.len() > 25 {
        reply.push_str("...and more.
");
    }

    msg.channel_id.say(&ctx.http, reply).await?;
    Ok(())
}

async fn maybe_fire_trigger(ctx: &Context, msg: &Message, state: &Arc<BotState>) -> Result<()> {
    let Some(guild_id) = msg.guild_id else {
        return Ok(());
    };

    let content = msg.content.to_lowercase();
    let triggers = {
        let stored = state.triggers.read().await;
        stored
            .servers
            .get(&guild_id.get().to_string())
            .cloned()
            .unwrap_or_default()
    };

    let Some(trigger) = triggers
        .into_iter()
        .find(|item| content.contains(&item.trigger.to_lowercase()))
    else {
        return Ok(());
    };

    let webhook = get_or_create_webhook(ctx, msg).await?;
    webhook
        .execute(
            &ctx.http,
            false,
            ExecuteWebhook::new()
                .content(trigger.response)
                .username(limit_webhook_username(&trigger.creator_name))
                .avatar_url(trigger.creator_avatar_url),
        )
        .await?;

    Ok(())
}

async fn get_or_create_webhook(ctx: &Context, msg: &Message) -> Result<Webhook> {
    let webhooks = msg.channel_id.webhooks(&ctx.http).await?;

    if let Some(webhook) = webhooks
        .into_iter()
        .find(|webhook| webhook.name.as_deref() == Some(WEBHOOK_NAME))
    {
        return Ok(webhook);
    }

    msg.channel_id
        .create_webhook(&ctx.http, CreateWebhook::new(WEBHOOK_NAME))
        .await
        .context("Could not create webhook. Give the bot Manage Webhooks in this channel.")
}

fn display_name(msg: &Message) -> String {
    msg.member
        .as_ref()
        .and_then(|member| member.nick.clone())
        .or_else(|| msg.author.global_name.clone())
        .unwrap_or_else(|| msg.author.name.clone())
}

fn limit_webhook_username(name: &str) -> &str {
    if name.len() <= 80 {
        return name;
    }

    let max = name
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= 80)
        .last()
        .unwrap_or(80);
    &name[..max]
}

async fn load_triggers(path: &PathBuf) -> Result<StoredTriggers> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => serde_json::from_str(&contents).context("Trigger storage is not valid JSON"),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(StoredTriggers::default()),
        Err(err) => Err(err).context("Could not read trigger storage"),
    }
}

async fn save_triggers(path: &PathBuf, triggers: &StoredTriggers) -> Result<()> {
    let contents = serde_json::to_string_pretty(triggers)?;
    tokio::fs::write(path, contents)
        .await
        .context("Could not write trigger storage")
}
