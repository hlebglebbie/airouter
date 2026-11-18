#!/usr/bin/env bash
# Поднимает airouter и Telegram-бота вместе.
#
#   ./dev/run.sh              # debug-сборка, сервер + бот
#   ./dev/run.sh --release    # release-сборка
#   ./dev/run.sh --no-bot     # только сервер (боту нужны зависимости из bot/requirements.txt)
#
# Переменные окружения прокидываются боту: AIROUTER_URL, TG_PROXY,
# TG_PROXY_COUNTRIES, TG_PROXY_ROTATE и т.д.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

PROFILE="debug"
WITH_BOT=1
for arg in "$@"; do
  case "$arg" in
    --release) PROFILE="release" ;;
    --no-bot) WITH_BOT=0 ;;
    *) echo "неизвестный аргумент: $arg" >&2; exit 2 ;;
  esac
done

LOCAL_URL="http://127.0.0.1:5959"
export AIROUTER_URL="${AIROUTER_URL:-$LOCAL_URL}"
VENV=".venv"
BOT_PY="$VENV/bin/python"

SERVER_PID=""
BOT_PID=""

cleanup() {
  trap - INT TERM EXIT
  for pid in "$BOT_PID" "$SERVER_PID"; do
    [[ -n "$pid" ]] && kill -TERM "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  echo
  echo "airouter: остановлен"
}
trap cleanup INT TERM EXIT

echo "==> сборка ($PROFILE)"
CARGO=(build --quiet)
if [[ "$PROFILE" == release ]]; then
  CARGO+=(--release)
fi
cargo "${CARGO[@]}"

echo "==> сервер на $LOCAL_URL"
"./target/$PROFILE/airouter" &
SERVER_PID=$!

for _ in {1..60}; do
  if curl -sf --max-time 1 "$LOCAL_URL/healthz" >/dev/null 2>&1; then break; fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "сервер завершился при старте" >&2
    exit 1
  fi
  sleep 0.5
done

if [[ "$WITH_BOT" == 1 ]]; then
  if [[ ! -x "$BOT_PY" ]]; then
    echo "==> создаю $VENV и ставлю зависимости бота"
    python3 -m venv "$VENV"
    if ! "$BOT_PY" -m pip install -q -r bot/requirements.txt; then
      echo "не удалось поставить зависимости — бот не запущен (--no-bot, чтобы скрыть это)" >&2
      WITH_BOT=0
    fi
  fi
fi

if [[ "$WITH_BOT" == 1 ]]; then
  echo "==> бот (прокси: ${TG_PROXY:-auto})"
  PYTHONUNBUFFERED=1 "$BOT_PY" bot/tg_bot.py &
  BOT_PID=$!
else
  echo "==> бот пропущен"
fi

echo
echo "дашборд: $LOCAL_URL  ·  endpoint: $LOCAL_URL/v1 (модели fast, think)"
echo "Ctrl+C остановит оба процесса"
echo

# Один процесс упал — останавливаем всё, чтобы не оставить бота без сервера.
status=0
wait -n "$SERVER_PID" ${BOT_PID:+"$BOT_PID"} || status=$?
echo "процесс завершился (код $status) — останавливаю остальные"
exit "$status"
