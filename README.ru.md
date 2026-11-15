[English](./README.en.md) · [Русский](./README.ru.md)

# airouter

OpenAI-совместимый роутер LLM с автофейловером и веб-интерфейсом. Один endpoint раздаёт две виртуальные модели — `fast` (быстрая) и `think` (умная) — и сам выбирает лучший из подключённых upstream-API по замерам скорости и размеру модели.

## Возможности

- OpenAI-совместимый endpoint: `/v1/chat/completions` и `/v1/models` (поддержка `stream`).
- Две виртуальные модели: `fast` → самый быстрый провайдер, `think` → самая крупная модель (по эвристике имени).
- Автоматическая проверка провайдеров: `/models` + тестовая генерация с замером задержки.
- Фейловер: при ошибке сразу переключается на следующего кандидата; «упавшие» провайдеры проверяются раз в 60 секунд и возвращаются в ротацию.
- Ручной выбор модели для каждого виртуального маршрута.
- Веб-дашборд на `/` для управления провайдерами, замерами и маршрутизацией.
- Сохранение настроек в `providers.json` (переживает рестарт).

## Сборка и запуск

Требуется Rust (stable).

```bash
cargo build --release
./target/release/airouter
```

Сервер слушает `http://127.0.0.1:5959` — откройте этот адрес в браузере, там веб-интерфейс.

Переменная окружения `AIROUTER_DATA_FILE` меняет путь к файлу с настройками (по умолчанию `providers.json` в рабочей директории).

## Использование endpoint

Клиент указывает `base_url = http://127.0.0.1:5959/v1` и модель `fast` или `think` (принимаются алиасы `gpt-fast`, `gpt-think` и др.).

```bash
curl http://127.0.0.1:5959/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"fast","messages":[{"role":"user","content":"привет"}],"stream":false}'
```

## Добавление провайдера

Через веб-интерфейс или API:

```bash
curl -X POST http://127.0.0.1:5959/api/providers \
  -H 'Content-Type: application/json' \
  -d '{"name":"my-api","base_url":"https://api.example.com/v1","api_key":"sk-...","model_override":null}'
```

## Admin API

- `GET /api/state` — состояние провайдеров и маршрутов
- `POST /api/providers` — добавить провайдера
- `PUT /api/providers/{id}` / `DELETE /api/providers/{id}` — изменить / удалить
- `POST /api/providers/{id}/check` / `POST /api/check_all` — перепроверить
- `POST /api/benchmark` — запустить «забег скорости»
- `PUT /api/routing` — выбрать модель для `fast` / `think`

## Безопасность

Сервер слушает только `127.0.0.1`. API-ключи хранятся в `providers.json` — не коммитьте этот файл и не открывайте порт наружу без reverse-proxy с аутентификацией.

## Лицензия

MIT — см. `LICENSE`.