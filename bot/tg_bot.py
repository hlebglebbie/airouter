#!/usr/bin/env python3
"""Telegram-бот airouter.

Читает токен бота из дашборда airouter (GET /api/telegram/token) и, когда в чат
присылают сообщение с base URL и API-ключами, добавляет каждый ключ в airouter
как провайдера через POST /api/providers.

Вокруг ключа и ссылки может быть любой текст — проза, эмодзи, Markdown, чужие
ссылки. Разбором занимается keyparser.py (чистый stdlib, тесты: bot/test_parser.py).

Запуск:
  pip install -r bot/requirements.txt
  python bot/tg_bot.py
  python3 bot/test_parser.py   # проверка парсера без внешних зависимостей

Переменные окружения:
  AIROUTER_URL   — адрес airouter (по умолчанию http://127.0.0.1:5959)
  TG_REFRESH_SECS — как часто проверять смену токена (по умолчанию 10)
  TG_PROXY       — `auto` (по умолчанию: брать прокси из free-proxy),
                   `off` (напрямую) или явный URL вида
                   http://user:pass@host:port
  TG_PROXY_COUNTRIES — страны для отбора прокси, через запятую (например DE,NL)
  TG_PROXY_ROTATE — срок жизни прокси в секундах, после чего берётся новый
                   (по умолчанию 300; 0 — без ротации)
  TG_PROXY_FALLBACK_DIRECT=1 — если free-proxy не дал прокси, идти напрямую
                             (по умолчанию — ждать и повторять)
"""
import asyncio
import os

import aiohttp
from aiogram import Bot, Dispatcher, F
from aiogram.client.session.aiohttp import AiohttpSession
from aiogram.exceptions import TelegramUnauthorizedError
from aiogram.filters import CommandStart
from aiogram.types import Message
from aiogram.utils.token import TokenValidationError, validate_token

from keyparser import parse, provider_name

AIROUTER_URL = os.environ.get("AIROUTER_URL", "http://127.0.0.1:5959").rstrip("/")
POLL_INTERVAL = float(os.environ.get("TG_REFRESH_SECS", "10"))

# Прокси для обращений к Telegram API. `auto` — каждый раз новый прокси из
# free-proxy (бесплатные списки живут минутами, поэтому ротация обязательна).
TG_PROXY = os.environ.get("TG_PROXY", "auto").strip()
PROXY_COUNTRIES = [
    c.strip().upper() for c in os.environ.get("TG_PROXY_COUNTRIES", "").split(",") if c.strip()
]
PROXY_ROTATE_SECS = float(os.environ.get("TG_PROXY_ROTATE", "300"))
PROXY_FALLBACK_DIRECT = os.environ.get("TG_PROXY_FALLBACK_DIRECT", "") == "1"
PROXY_RETRY_SECS = 15.0
# Бесплатные прокси часто оказываются мертвы уже в списке, поэтому после ошибки
# пробуем новый прокси почти сразу; в длинную паузу уходим только после серии
# неудач (чтобы не долбить список, когда источник прокси лежит).
FAST_RETRY_SECS = 1.0
MAX_FAST_RETRIES = 5


async def fetch_token(session: aiohttp.ClientSession) -> str:
    try:
        async with session.get(
            f"{AIROUTER_URL}/api/telegram/token",
            timeout=aiohttp.ClientTimeout(total=5),
        ) as resp:
            if resp.status == 200:
                data = await resp.json()
                return (data.get("token") or "").strip()
    except aiohttp.ClientError:
        pass
    return ""


async def add_providers(pairs):
    added = []
    errors = []
    async with aiohttp.ClientSession() as session:
        for token, base in pairs:
            name = provider_name(base)
            payload = {
                "name": name,
                "base_url": base,
                "api_key": token,
                "model_override": None,
            }
            try:
                async with session.post(
                    f"{AIROUTER_URL}/api/providers",
                    json=payload,
                    timeout=aiohttp.ClientTimeout(total=15),
                ) as resp:
                    if resp.status in (200, 201):
                        added.append(f"{name}  →  {mask(token)}")
                    else:
                        body = (await resp.text())[:200]
                        errors.append(f"{name}: HTTP {resp.status} {body}")
            except aiohttp.ClientError as exc:
                errors.append(f"{name}: {exc}")
    return added, errors


def mask(token: str) -> str:
    if len(token) <= 8:
        return token
    return f"{token[:5]}…{token[-4:]}"


async def on_start(message: Message):
    await message.answer(
        "Пришли сообщение с base URL и одним или несколькими API-ключами, например:\n\n"
        "https://api.example.com/v1\n"
        "sk-xxxxxxxxxxxxxxxxxxxx\n\n"
        "Я добавлю каждый ключ в airouter как провайдера."
    )


async def on_text(message: Message):
    result = parse(message.text or "")

    if not result.keys:
        await message.answer(
            "Не нашёл API-ключи в сообщении. Ожидаю ключи вида sk-…"
        )
        return

    sections: list[str] = []

    if result.pairs:
        added, errors = await add_providers(result.pairs)
        sections.append(f"Добавлено провайдеров: {len(added)}\n" + "\n".join(added))
        if errors:
            sections.append(f"Ошибок: {len(errors)}\n" + "\n".join(errors))

        # Ключ привязался к ссылке, не похожей на API-эндпоинт.
        api = set(result.api_urls)
        odd = sorted({base for _, base in result.pairs if base not in api})
        if odd:
            sections.append("⚠ нестандартный base URL (похоже, не API): " + ", ".join(odd))

    if result.orphans:
        keys = ", ".join(mask(k) for k in result.orphans)
        sections.append(
            f"Не добавлено — рядом нет base URL: {keys}\n"
            "Пришли ссылку вида https://api.example.com/v1"
        )

    await message.answer("\n\n".join(sections))


def build_dispatcher() -> Dispatcher:
    dp = Dispatcher()
    dp.message.register(on_start, CommandStart())
    dp.message.register(on_text, F.text)
    return dp


async def acquire_proxy() -> str | None:
    """Возвращает URL прокси для Telegram API либо None (работать напрямую).

    Режимы задаются переменной `TG_PROXY`. Бесплатные прокси из free-proxy
    недолговечны, поэтому вызывающий периодический опрос перебирает заново.
    """
    mode = TG_PROXY.lower()
    if mode == "off":
        return None
    if mode != "auto":
        return TG_PROXY  # явный URL

    # Импорт здесь, чтобы режимы `off` и явный URL работали без free-proxy.
    from fp.fp import AsyncFreeProxy, FreeProxyException

    try:
        return await AsyncFreeProxy(
            country_id=PROXY_COUNTRIES or None,
            https=True,  # api.telegram.org по HTTPS → нужен CONNECT-туннель
            rand=True,
            timeout=1.0,
        ).get()
    except FreeProxyException as exc:
        print(f"[airouter-tg] free-proxy: {exc}")
        return None
    except Exception as exc:
        print(f"[airouter-tg] free-proxy: не удалось получить прокси: {exc}")
        return None


async def _poll(bot: Bot) -> None:
    try:
        await bot.delete_webhook(drop_pending_updates=True)
    except TelegramUnauthorizedError:
        raise  # токен не принят — разбирается poll_forever, прокси тут ни при чём
    except Exception as exc:
        # Прокси/сеть: поллинг всё равно стартует и сам переберёт сбои get_updates
        # (aiogram крутит backoff, не выпуская исключение наружу).
        print(f"[airouter-tg] delete_webhook: {exc}")
    await build_dispatcher().start_polling(bot)


async def poll_forever(token: str) -> None:
    """Поллинг с ротацией прокси: каждый цикл — новый прокси и новая сессия."""
    try:
        validate_token(token)  # локально, до запроса прокси
    except TokenValidationError as exc:
        print(
            f"[airouter-tg] {exc} Ожидается вид `123456789:AAH…` — вероятно, токен\n"
            "                скопирован обрезанным или с пробелом. Исправь в дашборде."
        )
        return

    fails = 0
    while True:
        proxy = await acquire_proxy()
        if proxy is None and TG_PROXY.lower() == "auto" and not PROXY_FALLBACK_DIRECT:
            print(f"[airouter-tg] прокси нет — повтор через {PROXY_RETRY_SECS:.0f} с")
            await asyncio.sleep(PROXY_RETRY_SECS)
            continue

        bot = Bot(token=token, session=AiohttpSession(proxy=proxy))
        print(
            f"[airouter-tg] поллинг через прокси {proxy}"
            if proxy
            else "[airouter-tg] поллинг напрямую (прокси отключён)"
        )

        polling = asyncio.create_task(_poll(bot))
        rotate = (
            asyncio.create_task(asyncio.sleep(PROXY_ROTATE_SECS))
            if PROXY_ROTATE_SECS > 0
            else None
        )
        wait_on = [polling] + ([rotate] if rotate else [])

        try:
            await asyncio.wait(wait_on, return_when=asyncio.FIRST_COMPLETED)
        finally:
            for task in wait_on:
                if not task.done():
                    task.cancel()
            await asyncio.gather(*wait_on, return_exceptions=True)
            await bot.session.close()

        rotated = rotate is not None and rotate.done()
        exc = None if polling.cancelled() else polling.exception()

        if isinstance(exc, TelegramUnauthorizedError):
            # Формат верный, но Telegram такого бота не знает.
            print(
                "[airouter-tg] Telegram отверг токен (401) — бот удалён или токен пересоздан.\n"
                "                Возьми заново у @BotFather и сохрани в дашборде."
            )
            return

        if exc:
            fails += 1
            print(f"[airouter-tg] прокси {proxy} не выдержал ({fails}): {exc}")
        elif not rotated:
            fails += 1
            print("[airouter-tg] поллинг завершился сам, а не по ротации")
        else:
            # Прокси пережил полный цикл ротации — считаем его рабочим.
            fails = 0
            continue

        delay = FAST_RETRY_SECS if fails < MAX_FAST_RETRIES else PROXY_RETRY_SECS
        await asyncio.sleep(delay)


async def main() -> None:
    print(f"[airouter-tg] airouter: {AIROUTER_URL}")
    async with aiohttp.ClientSession() as session:
        current_token = None
        poll_task = None

        async def stop() -> None:
            nonlocal poll_task
            if poll_task is not None:
                poll_task.cancel()
                await asyncio.gather(poll_task, return_exceptions=True)
                poll_task = None

        while True:
            # Поллинг мог завершиться сам (например, Telegram отверг токен).
            # Сбрасыв состояние, чтобы повторное сохранение в дашборде перезапустило его.
            if poll_task is not None and poll_task.done():
                exc = None if poll_task.cancelled() else poll_task.exception()
                if exc:
                    print(f"[airouter-tg] поллинг остановлен: {exc}")
                poll_task = None
                current_token = None

            token = await fetch_token(session)
            if token != current_token:
                await stop()
                current_token = token
                if token:
                    poll_task = asyncio.create_task(poll_forever(token))
                    print("[airouter-tg] токен обновлён — запускаю поллинг")
                else:
                    print("[airouter-tg] токен не задан — жду настройки в дашборде")
            await asyncio.sleep(POLL_INTERVAL)


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except (KeyboardInterrupt, SystemExit):
        pass