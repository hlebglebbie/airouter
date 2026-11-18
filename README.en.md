[English](./README.en.md) · [Русский](./README.ru.md)

# airouter

An OpenAI-compatible LLM router with automatic failover, a web dashboard, and a Telegram bot for adding keys. A single endpoint exposes two virtual models — `fast` and `think` — and picks the best among the connected upstream APIs based on latency measurements and model size.

## Features

- OpenAI-compatible endpoint: `/v1/chat/completions` and `/v1/models` (streaming supported).
- Two virtual models: `fast` → the fastest provider, `think` → the largest model (by name heuristic).
- Automatic provider health checks: `/models` + a test generation with latency measurement.
- Failover: on error it immediately switches to the next candidate; failed providers are re-checked every 60 seconds and returned to rotation.
- Manual model selection for each virtual route.
- Web dashboard at `/` for managing providers, measurements, and routing.
- **Telegram bot**: forward it a message with keys and it extracts them plus the base URL, then adds the providers. The bot token is configured in the dashboard — no config edits, no restart.
- **Proxy support for the Telegram API** via [free-proxy](https://pypi.org/project/free-proxy/), for when `api.telegram.org` is unreachable directly.
- Settings are persisted to `providers.json` (survive restarts).

## Quick start

```bash
./dev/run.sh
```

The script builds the server, starts it, waits for `/healthz` to answer, then launches the bot. On the bot's first run it creates `.venv` and installs the dependencies from `bot/requirements.txt`. `Ctrl+C` stops both processes; if either one dies, the other is shut down too.

Flags: `--release` (release build), `--no-bot` (server only).

Requires Rust (stable), Python 3.11+ (what current aiogram needs), curl. To build without the script:

```bash
cargo build --release
./target/release/airouter
```

The server listens on `http://127.0.0.1:5959` — open that address in a browser for the web UI.

## Telegram bot

The bot exists only to add providers by forwarding a chat message. The router works perfectly without it — providers can be added through the dashboard or the Admin API.

**Setup.** Dashboard → "API" tab → "Telegram-бот" section (the dashboard UI is Russian) → paste the token from [@BotFather](https://t.me/BotFather) → Save. It is written to `providers.json`; the bot polls the server every 10 seconds and picks the change up on its own, no restart needed. "Отключить" (Disable) clears the token and the bot goes idle until a new one appears.

**What the bot does with a message.** From arbitrary text — prose, emoji, Markdown, channel reposts — it pulls out keys and links and matches them to each other:

```
repost 👇 https://t.me/mychannel
**https://api.samp.team/v1**
key `sk-EACv…xzqD` — works, tested ✅
```

→ provider `api.samp.team` added with base URL `https://api.samp.team/v1`.

Parsing rules (`bot/keyparser.py`, stdlib only):

- A key is `sk-` followed by at least 20 characters. Covers `sk-cp-81-…`, `sk-50x-…`, `sk-ant-…`; fragments like `sk-not-a-real-key` are rejected.
- A base URL is accepted with or without a scheme (`https://api.foo.com/v1`, `api.foo.com/v1`); endpoint suffixes are trimmed to the base (`/v1/chat/completions` → `/v1`), query and fragment are dropped, trailing punctuation and Markdown wrapping are stripped.
- Links to docs, social networks and trackers (`t.me`, `github.com`, `discord.gg`, `habr.com`, etc.) are never treated as a base — a key will not be attached to one.
- Each key gets the nearest API link before it, then after it, then any usable link. If the message contains no link at all, the key is not added: the bot names it and asks for a base URL.
- The provider name shown in the dashboard is the endpoint host.

**Proxy.** By default the bot takes proxies from free-proxy and reaches `api.telegram.org` through them. Free proxies are short-lived and frequently dead or MITM the TLS connection, so the bot rotates them: every cycle gets a new proxy and a new session; after a failure a fresh proxy is taken in 1 second, and only after five consecutive failures does it back off to 15 seconds. Token errors are separated from proxy failures — a malformed token or a 401 stops the rotation with a readable message instead of churning through nodes forever.

To reach Telegram directly or through your own proxy, set the environment variable when launching:

```bash
TG_PROXY=off ./dev/run.sh                       # direct
TG_PROXY=http://user:pass@host:3128 ./dev/run.sh   # your own proxy
TG_PROXY_COUNTRIES=DE,NL ./dev/run.sh              # filter by country
```

Requests to the local airouter bypass the proxy and use a separate session, so keys never transit third-party nodes.

## Using the endpoint

Point your client at `base_url = http://127.0.0.1:5959/v1` and use model `fast` or `think` (aliases like `gpt-fast`, `gpt-think` are also accepted).

```bash
curl http://127.0.0.1:5959/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"fast","messages":[{"role":"user","content":"hello"}],"stream":false}'
```

## Adding a provider

Via the web UI, the Telegram bot, or the API:

```bash
curl -X POST http://127.0.0.1:5959/api/providers \
  -H 'Content-Type: application/json' \
  -d '{"name":"my-api","base_url":"https://api.example.com/v1","api_key":"sk-...","model_override":null}'
```

Several keys of the same service can be added as separate entries; the router switches to a working one on its own. There is no deduplication: sending the same message to the bot twice creates two identical providers — remove the extra one manually in the dashboard.

## Admin API

- `GET /api/state` — provider and routing state (incl. `telegram`)
- `POST /api/providers` — add a provider
- `PUT /api/providers/{id}` / `DELETE /api/providers/{id}` — update / remove
- `POST /api/providers/{id}/check` / `POST /api/check_all` — re-check
- `POST /api/benchmark` — run a speed race
- `PUT /api/routing` — pick the model for `fast` / `think`
- `PUT /api/telegram` — set or clear the bot token (`{"token": "..."}`, empty string = disable)
- `GET /api/telegram/token` — the raw token for the bot; localhost only

## Environment variables

| Variable | For | Default | Meaning |
| --- | --- | --- | --- |
| `AIROUTER_DATA_FILE` | server | `providers.json` | settings file path |
| `AIROUTER_URL` | bot | `http://127.0.0.1:5959` | airouter address |
| `TG_REFRESH_SECS` | bot | `10` | how often to re-check the token |
| `TG_PROXY` | bot | `auto` | `auto` (free-proxy) · `off` (direct) · a proxy URL |
| `TG_PROXY_COUNTRIES` | bot | all | comma-separated, e.g. `DE,NL` |
| `TG_PROXY_ROTATE` | bot | `300` | proxy lifetime in seconds; `0` = no rotation |
| `TG_PROXY_FALLBACK_DIRECT` | bot | off | `1` — go direct if no proxy could be fetched |

## Tests

```bash
cargo test                     # server tests
python3 bot/test_parser.py     # key & base URL parsing checks
```

Both runners print their own check count. The parser tests need no dependencies — stdlib only, runnable with the system `python3`.

## Layout

```
src/                    server (axum): proxy, admin, benchmark, state, intel, upstream
static/                 dashboard (HTML/CSS/JS, embedded into the binary via include_str!)
bot/tg_bot.py           Telegram bot (aiogram)
bot/keyparser.py            key & base URL extraction from free text
bot/test_parser.py      its tests
dev/run.sh              start the server and the bot together
dev/upstream_stub.py    mock upstream for manual failover testing
providers.example.json  settings file template
```

## Security

The server only listens on `127.0.0.1`. Three things to know before exposing the port:

- **API keys** are stored in plaintext in `providers.json` — do not commit that file (it is already in `.gitignore`), and do not expose the port without an authenticating reverse proxy.
- **`GET /api/telegram/token`** returns the bot token with no authorization. Safe while the server is on localhost; close that route if you ever proxy the port outward.
- **The bot answers every sender.** There is no `chat_id`/`user_id` allowlist: anyone who learns the bot's username can add keys to your router. Keep the bot private or add a filter in `bot/tg_bot.py` (`on_text`) before giving it a public name.

The bot reaches Telegram through random free proxies. They cannot read request contents (traffic stays inside a TLS tunnel, certificates verified against `certifi`), but they do see the host and timing. For production, set your own vetted proxy via `TG_PROXY`.

## License

MIT — see `LICENSE`.