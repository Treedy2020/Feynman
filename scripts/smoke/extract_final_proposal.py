#!/usr/bin/env python3
"""Operator-only final reply extraction; preserves the original streamed evidence.

Run JSON comes from the public CLI. The Feynman-owned native JSONL is secondary
output provenance, not a substitute for public completion or source integrity.
Never prints the native log (which can contain private tool results).
"""
import argparse
import json
import os
from pathlib import Path


def final_reply(run_result, codex_home):
    run = run_result["run"]
    if run["status"] != "completed" or not run.get("runtime_turn_id"):
        raise ValueError("only an actually completed native turn can be extracted")
    matches = []
    for path in (codex_home / "sessions").rglob("*.jsonl"):
        with path.open() as stream:
            for line in stream:
                record = json.loads(line)
                payload = record.get("payload", {})
                if (record.get("type") == "event_msg"
                    and payload.get("type") == "task_complete"
                    and payload.get("turn_id") == run["runtime_turn_id"]):
                    content = payload.get("last_agent_message")
                    if not isinstance(content, str) or not content.strip():
                        raise ValueError("completed native turn has no final reply")
                    matches.append((path, content))
    if len(matches) != 1:
        raise ValueError("expected exactly one matching native completion")
    return matches[0]


def save_final(run_result, codex_home, output):
    source, content = final_reply(run_result, codex_home)
    # Keep all existing raw delta/proposal files. Refuse replacing final evidence.
    with open(output, "x", opener=lambda path, flags: os.open(path, flags, 0o600)) as stream:
        stream.write(content)
    return {"run_id": run_result["run"]["id"],
            "runtime_turn_id": run_result["run"]["runtime_turn_id"],
            "source": str(source), "output": str(output),
            "extraction": "matching native task_complete.last_agent_message",
            "raw_evidence_preserved": True}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--run-json", type=Path, required=True)
    parser.add_argument("--codex-home", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(save_final(json.loads(args.run_json.read_text()), args.codex_home,
                               args.output), ensure_ascii=False))


if __name__ == "__main__":
    main()
