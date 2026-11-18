#!/usr/bin/env python3
"""Парсер API-ключей и base URL из произвольного текста.

Сообщение боту почти никогда не состоит только из ключа и ссылки: вокруг
проза, эмодзи, разметка Markdown, репосты из чужих каналов, ссылки на доки.
Модуль выделяет из такого текста ключи и эндпоинты и сопоставляет их друг
другу.

Зависимостей нет намеренно (только stdlib) — парсер можно тестировать и
переиспользовать без aiogram/aiohttp.
"""
from __future__ import annotations

import re
from collections import namedtuple
from urllib.parse import urlparse

Hit = namedtuple("Hit", "pos end value")
Url = namedtuple("Url", "pos value api_like")
Parsed = namedtuple("Parsed", "pairs urls api_urls keys orphans")

# Ключ OpenAI-подобного сервиса: `sk-` и не менее 20 символов суффикса.
# [A-Za-z0-9_-] покрывает sk-cp-81-…, sk-50x-…, sk-ant-…; порог в 20 отсекает
# обрывки из прозы («sk-not-a-real-key» — 14 символов).
KEY_RE = re.compile(r"(?i)\bsk-[a-z0-9_\-]{20,}\b")

# Ссылка со схемой: путь необязателен (https://api.foo.com без /v1 — валидный
# эндпоинт, роутер сам допишет /v1).
SCHEME_RE = re.compile(r"https?://[^\s<>\"'`*)\],;]+")
# Хост без схемы: api.foo.com/v1. Путь обязателен — иначе матчится любое слово
# с точкой вроде «samp.team».
BARE_RE = re.compile(
    r"(?i)\b[a-z0-9][a-z0-9._-]*\.[a-z]{2,}(?::\d{2,5})?/[a-z0-9][a-z0-9._/~%-]*"
)

# Обрамление Markdown/скобок и концы предложений, прилипшие к значению.
TRAIL = ".,;:!?»«\"'`)"
# Суффиксы пути, которые являются частью эндпоинта, а не его базой.
ENDPOINT_SUFFIXES = (
    "/chat/completions",
    "/completions",
    "/embeddings",
    "/models",
    "/images/generations",
)
# Домены, на которые нельзя вешать API-ключ: соцсети, документация, трекеры.
DENIED_HOSTS = (
    "t.me",
    "telegram.org",
    "github.com",
    "gitlab.com",
    "discord.gg",
    "discord.com",
    "youtube.com",
    "twitter.com",
    "x.com",
    "habr.com",
    "reddit.com",
    "medium.com",
    "google.com",
    "wikipedia.org",
    "vk.com",
    "boosty.to",
)


def parse(text: str) -> Parsed:
    """Разбирает сообщение: возвращает `Parsed(pairs, urls, api_urls, keys, orphans)`.

    pairs    — [(ключ, base_url)] готовые к добавлению;
    urls     — все найденные базы, пригодные для ключа;
    api_urls — подмножество urls, похожее на API-эндпоинт;
    keys     — все найденные ключи (без дублей, в порядке появления);
    orphans  — ключи, для которых не нашлось ни одной подходящей базы.
    """
    urls = _scan_urls(text)
    keys = _dedupe(_scan_keys(text))

    pairs: list[tuple[str, str]] = []
    orphans: list[str] = []
    for pos, key in keys:
        base = _pick_base(pos, urls)
        if base is None:
            orphans.append(key)
        else:
            pairs.append((key, base))

    return Parsed(
        pairs=pairs,
        urls=[u.value for u in urls],
        api_urls=[u.value for u in urls if u.api_like],
        keys=[k for _, k in keys],
        orphans=orphans,
    )


def provider_name(base_url: str) -> str:
    """Короткое имя провайдера для дашборда — хост эндпоинта."""
    return urlparse(base_url).netloc or base_url


def normalize_base(raw: str) -> str:
    """Приводит сырое совпадение к базе: схема, без хвостовой пунктуации и суффикса эндпоинта."""
    value = raw.strip().strip(TRAIL).rstrip("/")
    value = value.split("#", 1)[0].split("?", 1)[0]  # query/fragment к эндпоинту не относятся
    if not value:
        return ""
    if "://" not in value:
        value = "https://" + value

    parts = urlparse(value)
    path = parts.path
    lowered = path.lower()
    for suffix in ENDPOINT_SUFFIXES:
        index = lowered.find(suffix)
        if index < 0:
            continue
        path = path[:index] if index > 0 else ""
        break

    scheme = parts.scheme or "https"
    host = parts.hostname or ""
    netloc = f"{host}:{parts.port}" if parts.port else host
    return f"{scheme}://{netloc}{path.rstrip('/')}"


def looks_like_api(base_url: str) -> bool:
    """Похожа ли база на эндпоинт OpenAI-совместимого API, а не на ссылку в доках."""
    parts = urlparse(base_url)
    host = (parts.hostname or "").lower()
    if not host or _denied(host):
        return False
    if "api" in host:
        return True
    path = parts.path.lower().rstrip("/")
    return bool(re.search(r"/v\d+(?:\.\w+)?$", path))


def _denied(host: str) -> bool:
    return any(host == d or host.endswith("." + d) for d in DENIED_HOSTS)


def _scan_urls(text: str) -> list[Url]:
    """Находит базы в порядке появления, без дублей и без заведомо чужих доменов."""
    found: list[Url] = []
    seen: set[str] = set()
    for hit in _find_urls(text):
        base = normalize_base(hit.value)
        if not base or base in seen:
            continue
        if _denied((urlparse(base).hostname or "").lower()):
            continue
        seen.add(base)
        found.append(Url(hit.pos, base, looks_like_api(base)))
    return found


def _scan_keys(text: str) -> list[tuple[int, str]]:
    """Находит ключи в тексте с «вырезанными» ссылками.

    Маскирование нужно и для корректности, и для порядка: в
    `sk-XXXXhttps://api.foo/v1` жадный класс символов съел бы «https» вместе
    со ссылкой, а после маски ключ обрывается на пробеле.
    """
    masked = _mask(text, _find_urls(text))
    return [(m.start(), m.group(0)) for m in KEY_RE.finditer(masked)]


def _find_urls(text: str) -> list[Hit]:
    """Сырые совпадения ссылок: сначала со схемой, затем голые хосты."""
    hits = [Hit(m.start(), m.end(), m.group(0)) for m in SCHEME_RE.finditer(text)]
    # Голый хост ищем уже после схемных: иначе «api.foo.com/v1» внутри
    # «https://api.foo.com/v1» дало бы второе совпадение.
    masked = _mask(text, hits)
    hits += [Hit(m.start(), m.end(), m.group(0)) for m in BARE_RE.finditer(masked)]
    hits.sort(key=lambda h: h.pos)
    return hits


def _dedupe(keys: list[tuple[int, str]]) -> list[tuple[int, str]]:
    seen: set[str] = set()
    out: list[tuple[int, str]] = []
    for pos, key in keys:
        if key not in seen:
            seen.add(key)
            out.append((pos, key))
    return out


def _pick_base(key_pos: int, urls: list[Url]) -> str | None:
    """База для ключа: ближайшая API-ссылка до него → после него → любая пригодная."""
    api = [u for u in urls if u.api_like]
    for pool, prefer_before in ((api, True), (api, False), (urls, True), (urls, False)):
        side = [u for u in pool if (u.pos < key_pos) == prefer_before]
        if side:
            edge = max if prefer_before else min
            return edge(side, key=lambda u: u.pos).value
    return None


def _mask(text: str, hits: list[Hit]) -> str:
    """Заменяет найденные ссылки пробелами, сохраняя смещения остального текста."""
    chars = list(text)
    for hit in hits:
        for i in range(hit.pos, min(hit.end, len(chars))):
            chars[i] = " "
    return "".join(chars)