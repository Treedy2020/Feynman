#!/usr/bin/env python3
"""Prepare private EDC evidence from existing DWS captures; no network or model calls.

This is a curated pilot, not a complete historical replay. Operator provenance,
future source inventories and scoring answers must never be seeded into an Agent.
"""
import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
SHANGHAI = timezone(timedelta(hours=8))
CASES = [
    {
        "id": "E-01",
        "base_sha": "6b1d6b601fd3b109d53db51b44f1355a7ef53825",
        "cutoff": "2026-09-08T17:30:00+08:00",
        "file": "edc-wangzichen-0826-0924.json",
        "ids": ["msg0R/2LNwq51K4AP7z6QDTBg=="],
    },
    {
        "id": "E-02",
        "base_sha": "b9328596b264f158f157b5001cd0cac9d84c775f",
        "cutoff": "2026-09-21T17:00:00+08:00",
        "file": "edc-business-0915-0924.json",
        "ids": [
            "msg5pEjK4YUcCutsChujCBVRg==", "msgpW61fr4pS3JezosUsklVLw==",
            "msg5CCRFWAJmegwHVxWZv0HHQ==", "msg2vAv3Z+48NHZGjg900/72w==",
            "msgZa4zBkAZW1NckLNbQ8fS7Q==", "msgbM4o1jekCqqcH3vqxwLpkA==",
        ],
    },
]


def sha(data):
    return hashlib.sha256(data).hexdigest()


def write_private(path, value):
    with open(path, "x", encoding="utf-8", opener=lambda p, flags: os.open(p, flags, 0o600)) as out:
        json.dump(value, out, ensure_ascii=False, indent=2)
        out.write("\n")


def collected_time(message):
    return datetime.strptime(message["createTime"], "%Y-%m-%d %H:%M:%S").replace(tzinfo=SHANGHAI)


def ledger(source):
    keys = ("count", "complete", "hasMore", "failures", "failedCount", "stopReason",
            "pagesFetched", "partial", "truncated", "truncatedByPageLimit", "truncatedByResultLimit")
    result = {key: source.get(key) for key in keys}
    downloads = source.get("resourceDownloads", {})
    result["resources"] = {key: downloads.get(key) for key in
                           ("discoveredCount", "downloadedCount", "failedCount", "failures", "partial", "ok")}
    if source.get("complete") is not True or source.get("hasMore") or source.get("failures"):
        raise ValueError("incomplete message capture; cannot label as complete")
    if source.get("partial") or source.get("truncated") or source.get("failedCount"):
        raise ValueError("partial/truncated capture")
    if len(source["messages"]) != source["count"]:
        raise ValueError("message count mismatch")
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    source_dir = args.source_dir.resolve()
    inventory_path = source_dir / "source-request-manifest.json"
    inventory_bytes = inventory_path.read_bytes()
    inventory = json.loads(inventory_bytes)
    # Fail closed: a new run creates a new directory; never overwrite prior evidence.
    args.output_dir.mkdir(mode=0o700, parents=True, exist_ok=False)
    agent_dir = args.output_dir / "agent"
    operator_dir = args.output_dir / "operator"
    agent_dir.mkdir(mode=0o700)
    operator_dir.mkdir(mode=0o700)
    report = {
        "schema_version": 1,
        "prepared_at": datetime.now(timezone.utc).isoformat(),
        "original_collection_date": inventory["collected_on"],
        "new_dws_query_performed": False,
        "model_run_performed": False,
        "purpose": "curated_EDC_evidence_preparation_not_full_history_or_completed_evaluation",
        "source_inventory_sha256": sha(inventory_bytes),
        "profile": inventory["profile"],
        "cases": [],
    }
    for case in CASES:
        query, = [q for q in inventory["queries"] if q["file"] == case["file"]]
        raw_path = source_dir / case["file"]
        raw_bytes = raw_path.read_bytes()
        source = json.loads(raw_bytes)
        source_ledger = ledger(source)
        cutoff = datetime.fromisoformat(case["cutoff"])
        start = datetime.fromisoformat(query["start"])
        end = datetime.fromisoformat(query["end"])
        if not start < cutoff <= end:
            raise ValueError("source window does not cover case cutoff")
        prior = [m for m in source["messages"] if start <= collected_time(m) < cutoff]
        projected = []
        selected = []
        assets = []
        for mid in case["ids"]:
            message, = [m for m in prior if m["messageId"] == mid]
            # Drop shared links (not retrieved attachments). Keep forwarded summary body.
            text = re.sub(r"\[https?://[^\n]+|\[dingtalk://[^\n]+",
                          "[分享链接省略；未获取其原始内容]", message["text"])
            projected.append({
                "messageId": mid, "conversationId": message["conversationId"],
                "created_at": collected_time(message).isoformat(),
                "sender": "需求方", "text": text,
                "attachments": [{"name": ref.get("name"), "status": "not_in_agent_input"}
                                for ref in message.get("resourceRefs") or []],
            })
            selected.append({
                "messageId": mid, "created_at": collected_time(message).isoformat(),
                "original_text_sha256": sha(message["text"].encode()),
                "projected_text_sha256": sha(text.encode()),
                "transformations": ["sender_display_pseudonymized", "shared_links_removed"],
            })
            resource_ids = {ref["resourceId"] for ref in message.get("resourceRefs") or []
                            if ref.get("resourceId")}
            for download in source.get("resourceDownloads", {}).get("downloads", []):
                # Some native download ledgers omit messageId. Join only exact
                # resourceId values from this message, never a guessed filename.
                if download.get("messageId") != mid and download.get("resourceId") not in resource_ids:
                    continue
                path = (ROOT / download["localPath"]).resolve()
                # Restrict operator asset inspection to this evidence tree.
                if not path.is_relative_to(source_dir):
                    raise ValueError("asset path outside evidence directory")
                asset_bytes = path.read_bytes()
                assets.append({"messageId": mid, "resourceId": download.get("resourceId"),
                               "join": "message.resourceRefs.resourceId == download.resourceId",
                               "name": path.name, "path": str(path),
                               "size_bytes": len(asset_bytes), "sha256": sha(asset_bytes),
                               "agent_visible": False, "historical_version_verified": False,
                               "content_audit": "pending"})
        facts = {
            "cutoff": case["cutoff"], "messages": projected,
            "selection": "operator-curated relevant messages; not the entire historical conversation",
            "reconstruction_limits": [
                "Historical edits and retractions are not reconstructed.",
                "Shared summary is not the original meeting recording/transcript.",
                "Attachments referenced in messages are not yet in this input.",
            ],
        }
        facts_path = agent_dir / (case["id"] + "-facts.json")
        write_private(facts_path, facts)
        scope = {"targets": [{"kind": "open_dingtalk_id" if query["target_kind"] == "openDingTalkId" else "group",
                              "id": query["target"]}], "start": query["start"], "end": case["cutoff"]}
        write_private(operator_dir / (case["id"] + "-scope.json"), scope)
        report["cases"].append({
            "case_id": case["id"], "base_sha": case["base_sha"], "cutoff": case["cutoff"],
            "raw_source_path": str(raw_path), "raw_source_sha256": sha(raw_bytes),
            "raw_query": query, "raw_ledger": source_ledger,
            "cutoff_candidate_count": len(prior), "selected_count": len(selected),
            "excluded_candidate_ids": [m["messageId"] for m in prior if m["messageId"] not in case["ids"]],
            "exclusion_reason": "not selected for curated pilot; contextual relevance not exhaustively audited",
            "selected_messages": selected, "assets": assets,
            "facts_path": str(facts_path.resolve()), "facts_sha256": sha(facts_path.read_bytes()),
            "ready": False, "remaining": ["owner_gold_calibration", "context_and_redaction_review",
                                            "isolated_baseline_and_runtime_env_preflight"] +
                                           (["attachment_content_and_version_audit"] if assets else []),
        })
    template = json.loads((ROOT / "docs/testing/edc-scorecard-template.json").read_text())
    write_private(operator_dir / "E-01-scorecard.json", template)
    write_private(operator_dir / "evidence-manifest.json", report)
    print(json.dumps({"output_dir": str(args.output_dir.resolve()), "new_dws_query_performed": False,
                      "cases": [{"id": c["case_id"], "selected": c["selected_count"],
                                 "cutoff_candidates": c["cutoff_candidate_count"],
                                 "assets": len(c["assets"]), "ready": c["ready"]} for c in report["cases"]]}))


if __name__ == "__main__":
    main()
