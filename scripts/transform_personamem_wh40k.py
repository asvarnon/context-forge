#!/usr/bin/env python3
"""Transform local PersonaMem 32k data into a WH40k/Mechanicus dialect variant.

This writes a parallel dataset tree and preserves benchmark gold linkage by deriving
`related_conversation_snippet` from the transformed chat-history turns, not by
separately rewriting the snippet text.

Expected endpoint: OpenAI-compatible /chat/completions.
"""

from __future__ import annotations

import argparse
import ast
import csv
import json
import os
import shutil
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

DEFAULT_IN_ROOT = Path("benchmarks/personamem/data")
DEFAULT_OUT_ROOT = Path("benchmarks/personamem/data_wh40k")
DEFAULT_BASE_URL = os.environ.get("OPENAI_BASE_URL", "http://localhost:1234/v1")
DEFAULT_MODEL = os.environ.get("OPENAI_MODEL", os.environ.get("MODEL", "local-model"))

PROMPT = r"""
PURPOSE — read before transforming

This text is being transformed to build an evaluation benchmark for a memory-retrieval
system. The benchmark measures whether a "domain lexicon" (a weighted vocabulary that
boosts importance-signalling words) helps a search engine surface the ONE conversation
turn where a user implicitly revealed a preference, out of a long history full of
distractor turns.

The source data (PersonaMem) is in plain English. We are re-skinning it into Warhammer
40,000 Imperial/Adeptus-Mechanicus dialect (a "Cawl Inferior" persona voice) WITHOUT
changing its structure, so we can test whether a WH40k-tuned lexicon helps on WH40k-
flavored data. The benchmark scores by exact-matching the evidence turn, and its whole
validity depends on three things you control:
  - The revealed preference must stay exactly as IMPLICIT as in the original.
  - The MEANING must be preserved exactly.
  - You must NOT know or infer which turn is "the important one."
Treat every turn as if it might be the evidence turn or a distractor; you cannot tell,
and you must not try.

TASK

You will receive a JSON array of turns, each {"i": <int>, "role": <str>, "content": <str>}.
Rewrite each turn's content into WH40k Imperial/Adeptus-Mechanicus dialect under the
rules below. Return ONLY a JSON array of {"i": <int>, "content": <rewritten str>} with
the SAME indices, in the SAME order, one object per input turn — no other keys, no prose.

HARD CONSTRAINTS — violating any invalidates the data:
1. PRESERVE MEANING EXACTLY. Every fact, preference, entity, number, and intent must
   survive unchanged. Do not add facts. Do not drop facts.
2. PRESERVE IMPLICITNESS. Keep indirectly-stated preferences exactly as indirect. NEVER
   make an implied preference more explicit, obvious, or easier to find. Do not
   summarize, flag, or emphasize any turn's "point."
3. PRESERVE STRUCTURE. Same role meaning, roughly the same length, same number of
   distinct statements, same conversational function per turn.
4. Judge each turn ONLY on its own text. You do NOT know and must NOT guess which turns
   matter. Treat none as special.
5. Preserve the index i of every turn exactly; return exactly as many objects as received.

REGISTER RULE:
- Vary devotional/reverent intensity by each turn's OWN emotional register, as a devout
  character naturally would: personal/heartfelt/committal/emphatic statements → heavier
  reverent language; neutral/transactional/procedural statements → light, plain reskin.
- Base this ONLY on how the speaker feels in that text.

VOCABULARY (draw naturally; never force terms where they don't fit):
- Reverence/importance: the Omnissiah, the God-Emperor, the Machine-God, the Throne,
  blessed, sacred, sanctified, by the Motive Force, praise be, it is the Emperor's will.
- Commitment/affirmation: "I so vow," "it shall be done," "by the Omnissiah I affirm,"
  "let it be recorded."
- Tech/framing: cogitator, data-slate, machine-spirit, augur, vox, lexmechanic, rites, litany.
- Negation/doubt: "the Emperor frowns upon this," "corrupted by the warp," "the
  machine-spirit is silent."

OUTPUT: ONLY the JSON array described above. No markdown fences, no commentary.
""".strip()


@dataclass(frozen=True)
class TurnRef:
    role: str
    content: str
    history_index: int


class OpenAICompatClient:
    def __init__(self, base_url: str, api_key: str, model: str, timeout: int, temperature: float):
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.model = model
        self.timeout = timeout
        self.temperature = temperature

    def transform_batch(self, items: list[dict[str, Any]], retries: int = 4) -> dict[int, str]:
        body = {
            "model": self.model,
            "temperature": self.temperature,
            "messages": [
                {"role": "system", "content": PROMPT},
                {"role": "user", "content": json.dumps(items, ensure_ascii=False)},
            ],
        }
        data = json.dumps(body, ensure_ascii=False).encode("utf-8")
        req = urllib.request.Request(
            f"{self.base_url}/chat/completions",
            data=data,
            headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {self.api_key}",
            },
            method="POST",
        )

        last_err: Exception | None = None
        for attempt in range(retries + 1):
            try:
                with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                    payload = json.loads(resp.read().decode("utf-8"))
                text = payload["choices"][0]["message"]["content"].strip()
                parsed = parse_json_array(text)
                return validate_response(items, parsed)
            except Exception as e:  # noqa: BLE001 - retain endpoint/parser errors for retry context.
                last_err = e
                if attempt == retries:
                    break
                time.sleep(2**attempt)
        raise RuntimeError(f"LLM batch failed after {retries + 1} attempts: {last_err}")


def parse_json_array(text: str) -> list[Any]:
    if text.startswith("```"):
        lines = text.splitlines()
        if lines and lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].startswith("```"):
            lines = lines[:-1]
        text = "\n".join(lines).strip()
    return json.loads(text)


def validate_response(items: list[dict[str, Any]], parsed: list[Any]) -> dict[int, str]:
    if not isinstance(parsed, list):
        raise ValueError("response is not a JSON array")
    expected = [int(x["i"]) for x in items]
    got: list[int] = []
    out: dict[int, str] = {}
    for obj in parsed:
        if not isinstance(obj, dict) or set(obj.keys()) != {"i", "content"}:
            raise ValueError(f"bad response object: {obj!r}")
        i = int(obj["i"])
        if not isinstance(obj["content"], str):
            raise ValueError(f"content for {i} is not a string")
        got.append(i)
        out[i] = obj["content"]
    if got != expected:
        raise ValueError(f"index/order mismatch: expected {expected}, got {got}")
    return out


def batched(xs: list[dict[str, Any]], n: int):
    for start in range(0, len(xs), n):
        yield xs[start : start + n]


def transform_items(client: OpenAICompatClient, items: list[dict[str, Any]], batch_size: int) -> dict[int, str]:
    transformed: dict[int, str] = {}
    for batch in batched(items, batch_size):
        transformed.update(client.transform_batch(batch))
    return transformed


def load_history(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def find_snippet_refs(history: list[dict[str, str]], snippet_text: str) -> list[TurnRef]:
    turns = json.loads(snippet_text)
    wanted = [(t.get("role", ""), t["content"]) for t in turns]

    # Prefer an exact contiguous role+content match, preserving the snippet sequence.
    for start in range(0, len(history) - len(wanted) + 1):
        if all(
            history[start + off].get("role") == role and history[start + off].get("content") == content
            for off, (role, content) in enumerate(wanted)
        ):
            return [TurnRef(role, content, start + off) for off, (role, content) in enumerate(wanted)]

    # Fallback: exact role+content per snippet turn. This handles non-contiguous annotations.
    refs: list[TurnRef] = []
    used: set[int] = set()
    for role, content in wanted:
        for idx, msg in enumerate(history):
            if idx not in used and msg.get("role") == role and msg.get("content") == content:
                refs.append(TurnRef(role, content, idx))
                used.add(idx)
                break
        else:
            raise ValueError("snippet turn not found in history")
    return refs


def parse_user_query_cell(cell: str) -> tuple[Any, str | None]:
    """Return parsed cell and content if cell is a {'role','content'} literal/JSON."""
    for loader in (json.loads, ast.literal_eval):
        try:
            obj = loader(cell)
            if isinstance(obj, dict) and isinstance(obj.get("content"), str):
                return obj, obj["content"]
        except Exception:
            pass
    return cell, cell


def dump_user_query_cell(original_obj: Any, transformed_content: str) -> str:
    if isinstance(original_obj, dict) and "content" in original_obj:
        obj = dict(original_obj)
        obj["content"] = transformed_content
        # Input CSV uses Python-literal dicts; preserving that shape avoids changing parser assumptions.
        return repr(obj)
    return transformed_content


def transform_csv_fields_for_persona(
    client: OpenAICompatClient,
    rows: list[dict[str, str]],
    batch_size: int,
) -> None:
    items: list[dict[str, Any]] = []
    metadata: dict[int, tuple[dict[str, str], str, int | None]] = {}
    next_i = 0

    for row in rows:
        parsed_query, query_content = parse_user_query_cell(row["user_query"])
        items.append({"i": next_i, "role": "user", "content": query_content})
        metadata[next_i] = (row, "user_query", None)
        row["__parsed_user_query"] = parsed_query  # type: ignore[assignment]
        next_i += 1

        items.append({"i": next_i, "role": "assistant", "content": row["correct_answer"]})
        metadata[next_i] = (row, "correct_answer", None)
        next_i += 1

        incorrect = json.loads(row["incorrect_answers"])
        row["__incorrect_answers"] = incorrect  # type: ignore[assignment]
        for j, answer in enumerate(incorrect):
            items.append({"i": next_i, "role": "assistant", "content": answer})
            metadata[next_i] = (row, "incorrect_answers", j)
            next_i += 1

    # Keep each row's user_query + correct + three incorrect answers in the same LLM call.
    # PersonaMem has exactly 3 incorrect options, so each row contributes 5 items.
    row_width = 5
    grouped_batch_size = max(1, batch_size // row_width) * row_width
    transformed = transform_items(client, items, grouped_batch_size)

    for i, text in transformed.items():
        row, field, offset = metadata[i]
        if field == "user_query":
            parsed = row.pop("__parsed_user_query")  # type: ignore[arg-type]
            row["user_query"] = dump_user_query_cell(parsed, text)
        elif field == "correct_answer":
            row["correct_answer"] = text
        elif field == "incorrect_answers":
            answers = row["__incorrect_answers"]  # type: ignore[index]
            answers[offset] = text  # type: ignore[index]
        else:
            raise AssertionError(field)

    for row in rows:
        if "__incorrect_answers" in row:
            row["incorrect_answers"] = json.dumps(row.pop("__incorrect_answers"), ensure_ascii=False)  # type: ignore[arg-type]
        row.pop("__parsed_user_query", None)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--input-root", type=Path, default=DEFAULT_IN_ROOT)
    ap.add_argument("--output-root", type=Path, default=DEFAULT_OUT_ROOT)
    ap.add_argument("--base-url", default=DEFAULT_BASE_URL)
    ap.add_argument("--api-key", default=os.environ.get("OPENAI_API_KEY", "not-needed"))
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--batch-size", type=int, default=25)
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--temperature", type=float, default=0.2)
    ap.add_argument("--limit-personas", type=int)
    ap.add_argument("--force", action="store_true", help="allow replacing an existing output tree")
    ap.add_argument(
        "--fail-missing-snippet",
        action="store_true",
        help="fail instead of skipping rows whose gold snippet is not present in the local 32k history",
    )
    args = ap.parse_args()

    in_csv = args.input_root / "benchmark.csv"
    out_csv = args.output_root / "benchmark.csv"
    in_history_root = args.input_root / "data" / "chat_history_32k"
    out_history_root = args.output_root / "data" / "chat_history_32k"

    if args.output_root.exists():
        if not args.force:
            raise SystemExit(f"output root already exists: {args.output_root} (use --force to replace)")
        shutil.rmtree(args.output_root)
    out_history_root.mkdir(parents=True, exist_ok=True)

    with in_csv.open("r", encoding="utf-8", newline="") as f:
        reader = csv.DictReader(f)
        fieldnames = reader.fieldnames
        if fieldnames is None:
            raise SystemExit("benchmark.csv has no header")
        all_rows = list(reader)

    existing_links: set[str] = set()
    for row in all_rows:
        link = row["chat_history_32k_link"]
        if (args.input_root / link).exists():
            existing_links.add(link)

    links = sorted(existing_links)
    if args.limit_personas is not None:
        links = links[: args.limit_personas]
    link_set = set(links)

    rows_by_link: dict[str, list[dict[str, str]]] = {link: [] for link in links}
    for row in all_rows:
        link = row["chat_history_32k_link"]
        if link in link_set:
            rows_by_link[link].append(row)

    client = OpenAICompatClient(args.base_url, args.api_key, args.model, args.timeout, args.temperature)
    print(f"Transforming {len(links)} local personas into {args.output_root}", file=sys.stderr)

    included_ids: set[int] = set()
    skipped_missing_snippet = 0
    for n, link in enumerate(links, start=1):
        in_path = args.input_root / link
        out_path = args.output_root / link
        history_obj = load_history(in_path)
        history = history_obj["chat_history"]
        persona_rows = rows_by_link[link]

        # Validate and store gold refs before spending LLM calls on rows that cannot be scored
        # against the local 32k history. Some locally available personas still have rows whose
        # annotated evidence turn only exists in the 128k history.
        valid_rows: list[dict[str, str]] = []
        row_refs: dict[int, list[TurnRef]] = {}
        for row in persona_rows:
            try:
                row_refs[id(row)] = find_snippet_refs(history, row["related_conversation_snippet"])
                valid_rows.append(row)
            except Exception as e:  # noqa: BLE001 - choose skip/fail from CLI.
                if args.fail_missing_snippet:
                    raise RuntimeError(
                        f"gold snippet for persona {row['persona_id']} not found in {link}"
                    ) from e
                skipped_missing_snippet += 1

        print(
            f"[{n}/{len(links)}] {link}: {len(history)} messages, "
            f"{len(valid_rows)}/{len(persona_rows)} scorable rows",
            file=sys.stderr,
        )

        items = [
            {"i": i, "role": msg.get("role", ""), "content": msg.get("content", "")}
            for i, msg in enumerate(history)
        ]
        transformed_history = transform_items(client, items, args.batch_size)

        for i, msg in enumerate(history):
            msg["content"] = transformed_history[i]
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(history_obj, ensure_ascii=False, indent=2), encoding="utf-8")

        transform_csv_fields_for_persona(client, valid_rows, args.batch_size)

        # Relink gold snippets from transformed history turns. Never free-rewrite this field.
        for row in valid_rows:
            refs = row_refs[id(row)]
            row["related_conversation_snippet"] = json.dumps(
                [
                    {
                        "role": ref.role,
                        "content": transformed_history[ref.history_index],
                    }
                    for ref in refs
                ],
                ensure_ascii=False,
            )
            included_ids.add(id(row))

    output_rows = [{k: row[k] for k in fieldnames} for row in all_rows if id(row) in included_ids]

    out_csv.parent.mkdir(parents=True, exist_ok=True)
    with out_csv.open("w", encoding="utf-8", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(output_rows)

    print(f"Wrote {len(output_rows)} rows: {out_csv}", file=sys.stderr)
    if skipped_missing_snippet:
        print(f"Skipped {skipped_missing_snippet} rows whose gold snippet is absent from local 32k histories", file=sys.stderr)
    print(f"Wrote histories: {out_history_root}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
