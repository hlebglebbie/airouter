[English](./README.en.md) · [Русский](./README.ru.md)

# airouter

An OpenAI-compatible LLM router with automatic failover and a web dashboard. A single endpoint exposes two virtual models — `fast` and `think` — and picks the best among the connected upstream APIs based on latency measurements and model size.

## Features

- OpenAI-compatible endpoint: `/v1/chat/completions` and `/v1/models` (streaming supported).
- Two virtual models: `fast` → the fastest provider, `think` → the largest model (by name heuristic).
- Automatic provider health checks: `/models` + a test generation with latency measurement.
- Failover: on error it immediately switches to the next candidate; failed providers are re-checked every 60 seconds and returned to rotation.
- Manual model selection for each virtual route.
- Web dashboard at `/` for managing providers, measurements, and routing.
- Settings are persisted to `providers.json` (survive restarts).

## Build & run

Requires Rust (stable).

```bash
cargo build --release
./target/release/airouter
```

The server listens on `http://127.0.0.1:5959` — open that address in a browser for the web UI.

The `AIROUTER_DATA_FILE` environment variable changes the settings file path (defaults to `providers.json` in the working directory).

## Using the endpoint

Point your client at `base_url = http://127.0.0.1:5959/v1` and use model `fast` or `think` (aliases like `gpt-fast`, `gpt-think` are also accepted).

```bash
curl http://127.0.0.1:5959/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"fast","messages":[{"role":"user","content":"hello"}],"stream":false}'
```

## Adding a provider

Via the web UI or the API:

```bash
curl -X POST http://127.0.0.1:5959/api/providers \
  -H 'Content-Type: application/json' \
  -d '{"name":"my-api","base_url":"https://api.example.com/v1","api_key":"sk-...","model_override":null}'
```

## Admin API

- `GET /api/state` — provider and routing state
- `POST /api/providers` — add a provider
- `PUT /api/providers/{id}` / `DELETE /api/providers/{id}` — update / remove
- `POST /api/providers/{id}/check` / `POST /api/check_all` — re-check
- `POST /api/benchmark` — run a speed race
- `PUT /api/routing` — pick the model for `fast` / `think`

## Security

The server only listens on `127.0.0.1`. API keys are stored in `providers.json` — do not commit that file, and do not expose the port without an authenticating reverse proxy.

## License

MIT — see `LICENSE`.