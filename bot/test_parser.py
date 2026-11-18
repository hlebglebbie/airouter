#!/usr/bin/env python3
"""Тесты парсера ключей и base URL на сообщениях с посторонним текстом.

Запуск: python3 bot/test_parser.py
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from keyparser import looks_like_api, normalize_base, parse, provider_name  # noqa: E402

FAILURES: list[str] = []


def check(name: str, got, want) -> None:
    if got != want:
        FAILURES.append(f"{name}\n     got: {got!r}\n    want: {want!r}")


def pairs_of(text: str):
    return parse(text).pairs


def main() -> int:
    # --- чистое сообщение -----------------------------------------------------
    check(
        "чистое: url + ключ",
        pairs_of("https://api.foo.com/v1\nsk-AAAAAAAAAAAAAAAAAAAAAAAA"),
        [("sk-AAAAAAAAAAAAAAAAAAAAAAAA", "https://api.foo.com/v1")],
    )

    # --- проза вокруг ---------------------------------------------------------
    check(
        "проза после ключа",
        pairs_of("https://api.foo.com/v1\nsk-AAAAAAAAAAAAAAAAAAAAAAAA — рабочий, всем советую"),
        [("sk-AAAAAAAAAAAAAAAAAAAAAAAA", "https://api.foo.com/v1")],
    )
    check(
        "проза+эмодзи+ключ до ссылки",
        pairs_of("ребят 🔥 sk-BBBBBBBBBBBBBBBBBBBBBBBB это ключ для https://api.bar.com/v1"),
        [("sk-BBBBBBBBBBBBBBBBBBBBBBBB", "https://api.bar.com/v1")],
    )
    check(
        "метки key:/token:/base_url:",
        pairs_of("base_url: https://api.lab.com/v1\ntoken: sk-CCCCCCCCCCCCCCCCCCCCCCCC"),
        [("sk-CCCCCCCCCCCCCCCCCCCCCCCC", "https://api.lab.com/v1")],
    )

    # --- разметка -------------------------------------------------------------
    check(
        "markdown-обрамление",
        pairs_of("`https://api.qux.com/v1` + `sk-DDDDDDDDDDDDDDDDDDDDDD`"),
        [("sk-DDDDDDDDDDDDDDDDDDDDDD", "https://api.qux.com/v1")],
    )
    check(
        "жирный и скобки",
        pairs_of("**https://api.bold.com/v1** (sk-EEEEEEEEEEEEEEEEEEEEEEEE)"),
        [("sk-EEEEEEEEEEEEEEEEEEEEEEEE", "https://api.bold.com/v1")],
    )
    check(
        "блок кода с языком",
        pairs_of("```text\nhttps://api.fence.com/v1\nsk-FFFFFFFFFFFFFFFFFFFFFFFFFF\n```"),
        [("sk-FFFFFFFFFFFFFFFFFFFFFFFFFF", "https://api.fence.com/v1")],
    )

    # --- чужие ссылки ---------------------------------------------------------
    parsed = parse("см. https://t.me/mychannel и https://api.baz.com/v1\nsk-GGGGGGGGGGGGGGGGGGGGGG")
    check("ссылка на канал отброшена", parsed.urls, ["https://api.baz.com/v1"])
    check("ключ привязан к API", parsed.pairs, [("sk-GGGGGGGGGGGGGGGGGGGGGG", "https://api.baz.com/v1")])

    orphan = parse("обсудили тут https://github.com/foo/bar/issues/12\nsk-HHHHHHHHHHHHHHHHHHHHHH")
    check("ключ не вешается на github", orphan.pairs, [])
    check("ключ ушёл в orphans", orphan.orphans, ["sk-HHHHHHHHHHHHHHHHHHHHHH"])

    # --- несколько пар --------------------------------------------------------
    check(
        "две ссылки и два ключа вперемешку с текстом",
        pairs_of(
            "первый https://api.one.com/v1\nключ sk-1111111111111111111111\n"
            "второй https://api.two.com/v1\nключ sk-2222222222222222222222"
        ),
        [
            ("sk-1111111111111111111111", "https://api.one.com/v1"),
            ("sk-2222222222222222222222", "https://api.two.com/v1"),
        ],
    )
    check(
        "несколько ключей на одну ссылку",
        pairs_of("https://api.multi.com/v1\nsk-3333333333333333333333\nsk-4444444444444444444444"),
        [
            ("sk-3333333333333333333333", "https://api.multi.com/v1"),
            ("sk-4444444444444444444444", "https://api.multi.com/v1"),
        ],
    )
    check("дубликат ключа схлопывается", len(pairs_of("https://api.dup.com/v1\nsk-5555555555555555555555\nsk-5555555555555555555555")), 1)

    # --- форма ссылки --------------------------------------------------------
    check(
        "суффикс эндпоинта срезан до /v1",
        normalize_base("https://api.cut.com/v1/chat/completions"),
        "https://api.cut.com/v1",
    )
    check(
        "/models срезан",
        normalize_base("https://api.models.com/v1/models"),
        "https://api.models.com/v1",
    )
    check(
        "голый хост без схемы",
        pairs_of("api.samp.team/v1\nsk-6666666666666666666666"),
        [("sk-6666666666666666666666", "https://api.samp.team/v1")],
    )
    check(
        "хост без пути",
        normalize_base("https://api.noprefix.com"),
        "https://api.noprefix.com",
    )
    check("порт сохранён", normalize_base("http://192.168.1.5:8000/v1"), "http://192.168.1.5:8000/v1")
    check("query отброшен", normalize_base("https://api.q.com/v1?free=1"), "https://api.q.com/v1")
    check("точка в конце предложения", normalize_base("https://api.dot.com/v1."), "https://api.dot.com/v1")
    check("слово с точкой не ссылка", parse("samp.team просто текст sk-7777777777777777777777").urls, [])

    # --- склеенные значения ---------------------------------------------------
    check(
        "ключ приклеен к ссылке",
        pairs_of("sk-8888888888888888888888https://api.glue.com/v1"),
        [("sk-8888888888888888888888", "https://api.glue.com/v1")],
    )
    check(
        "через запятую без пробела",
        pairs_of("https://api.comma.com/v1,sk-9999999999999999999999"),
        [("sk-9999999999999999999999", "https://api.comma.com/v1")],
    )

    # --- мусор ----------------------------------------------------------------
    check("короткий sk-фрагмент игнорируется", parse("https://api.zzz.com/v1 sk-not-a-real-key").keys, [])
    check("ключа нет", parse("https://api.empty.com/v1 привет").pairs, [])
    no_url = parse("мой ключ sk-AAAAAAAAAAAAAAAA22222222")
    check("ссылки нет → orphan", (no_url.pairs, no_url.orphans), ([], ["sk-AAAAAAAAAAAAAAAA22222222"]))

    # --- реальные ключи из providers.json -------------------------------------
    check(
        "реальный samp.team",
        pairs_of(
            "держи https://api.samp.team/v1 sk-EACv5bDTsLErYxhLHpYK2Q6HGWQa1qHv72qdX2YxzqD "
            "и не свети никому"
        ),
        [("sk-EACv5bDTsLErYxhLHpYK2Q6HGWQa1qHv72qdX2YxzqD", "https://api.samp.team/v1")],
    )
    minimax = (
        "sk-cp-81-0EOBFVG4baU-E7ewS5v-zEASRFIda4r9YfqQShV2ifgwd_53jsV0Bnt6HiD4xsLlI"
        "iq71cnAO_IoFDJwIMn5NB3WWiuv8CBv61e4IVZ2_KRmuTUynZZU"
    )
    check(
        "реальный minimax со sk-cp-",
        pairs_of(f"https://api.minimax.io/v1\n{minimax}\nпроверено, пашет"),
        [(minimax, "https://api.minimax.io/v1")],
    )
    # Ключ, разорванный переводом строки, намеренно не склеивается: иначе
    # любое перенесённое предложение выглядело как ключ.
    check(
        "ключ в две строки не склеивается",
        parse("sk-AAAAAAAAAAAA\nBBBBBBBBBBBB").keys,
        [],
    )

    # --- вспомогательные ------------------------------------------------------
    check("provider_name", provider_name("https://api.samp.team/v1"), "api.samp.team")
    check("looks_like_api: /v1", looks_like_api("https://foo.example/v1"), True)
    check("looks_like_api: api-хост", looks_like_api("https://api.foo.com"), True)
    check("looks_like_api: t.me", looks_like_api("https://t.me/chan"), False)
    check("normalize пустого", normalize_base("   "), "")

    if FAILURES:
        print(f"ПАДЕНИЕ: {len(FAILURES)}\n")
        for f in FAILURES:
            print(" ✗ " + f)
        return 1
    print("все проверки парсера прошли")
    return 0


if __name__ == "__main__":
    sys.exit(main())