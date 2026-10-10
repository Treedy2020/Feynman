#!/usr/bin/env python3
"""Operator-only bounded selection from already authorized DWS evidence.

Writes private input packs, not gold/answers. Exclusions and reconstruction limits
are recorded. No new live DWS queries and no credentials are migrated.
"""
import argparse
from datetime import datetime, timezone, timedelta
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]

def write(path, value):
    with open(path, "x", opener=lambda p, f: os.open(p, f, 0o600)) as out:
        json.dump(value, out, ensure_ascii=False)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    args.output_dir.mkdir(mode=0o700, parents=True, exist_ok=False)
    configs = [
        {"case_id":"E-01","repository":ROOT.parent/"EdcDataGeneration","base_sha":"6b1d6b601fd3b109d53db51b44f1355a7ef53825","cutoff":"2026-09-08T17:30:00+08:00","file":"edc-wangzichen-0826-0924.json","ids":["msg0R/2LNwq51K4AP7z6QDTBg=="],"excludes":[".claude","AGENTS.md","CLAUDE.md","agent-skills","plugin/.claude","docs","plugin/tests/regression/fixtures","plugin/tests/v2/fixtures"]},
        {"case_id":"C-01","repository":ROOT.parent/"Cohort","base_sha":"a8cd723c4f420ff18f487cdf1bb4b12d4df54bbc","cutoff":"2026-09-22T14:05:00+08:00","file":"cohort-yang-0831-0923.json","ids":["msgBj4ewFX46nq4s17ramvqTw==","msgTUB5hOV9KJKuReKQwGHgFw==","msgJokGmK6mrJ0YwKEE4cCTOA=="],"excludes":["reference-materials","docs","frontend/docs",".env.example","AGENTS.md","CLAUDE.md","skills","backend/tests/core/test_settings_dsn.py"]},
    ]
    manifest = {"purpose":"curated_direction_smoke_not_full_historical_eval","cases":[]}
    for config in configs:
        raw = json.loads((args.evidence/config["file"]).read_text())
        if raw.get("complete") is not True or raw.get("hasMore") or raw.get("failures"):
            raise RuntimeError("source query incomplete")
        cutoff = datetime.fromisoformat(config["cutoff"])
        messages = []
        for identifier in config["ids"]:
            matched = [m for m in raw["messages"] if m.get("messageId") == identifier]
            if len(matched) != 1:
                raise RuntimeError("message identity unavailable/ambiguous")
            message = matched[0]
            created = datetime.strptime(message["createTime"], "%Y-%m-%d %H:%M:%S").replace(tzinfo=timezone(timedelta(hours=8)))
            if created >= cutoff:
                raise RuntimeError("future source rejected")
            text = re.sub(r"\[https?://[^\n]+|\[dingtalk://[^\n]+", "[分享链接省略；已保留用户转发正文]", message["text"])
            messages.append({"messageId":identifier,"conversationId":message["conversationId"],"created_at":created.isoformat(),"sender":"需求方","text":text})
        facts = {"cutoff":config["cutoff"],"messages":messages,"selection":"operator-curated relevant messages; not the entire historical conversation",
                 "reconstruction_limits":["historical edits/retractions are not reconstructed","EDC meeting original transcript is not retrieved; forwarded summary is the evidence"]}
        if config["case_id"] == "C-01":
            doc = args.evidence/"yang-resources/Cohort 邮件回复模板.docx"
            original = subprocess.check_output(["textutil","-convert","txt","-stdout",str(doc)],text=True)
            # Clinical examples are unnecessary for this bounded software-direction smoke.
            template = original.split("示例一：",1)[0].strip()
            facts["attachments"] = [{"messageId":config["ids"][-1],"name":"Cohort 邮件回复模板.docx","original_sha256":hashlib.sha256(doc.read_bytes()).hexdigest(),"text":template,
                                     "transformation":"textutil extraction; clinical examples omitted to avoid unnecessary participant data"}]
        facts_path = args.output_dir/(config["case_id"]+"-facts.json")
        write(facts_path, facts)
        snapshot = args.output_dir/(config["case_id"]+"-snapshot.json")
        command = [str(ROOT/"target/debug/feynman"),"evaluation","prepare","--repository",str(config["repository"]),"--base-sha",config["base_sha"],"--facts",str(facts_path),"--output",str(snapshot)]
        for prefix in config["excludes"]:
            command += ["--exclude-prefix",prefix]
        result = subprocess.run(command,text=True,capture_output=True,timeout=120)
        if result.returncode:
            raise RuntimeError(result.stderr)
        summary = json.loads(result.stdout)
        # Fail closed on common embedded credential patterns; only paths are reported.
        import base64
        suspicious = []
        for file in json.loads(snapshot.read_text())["files"]:
            text = base64.b64decode(file["content_base64"]).decode("utf-8",errors="ignore")
            if re.search(r"(?:sk-[A-Za-z0-9_-]{20,}|glpat-[A-Za-z0-9_-]{15,}|-----BEGIN (?:RSA |OPENSSH )?PRIVATE KEY-----|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})",text):
                suspicious.append(file["path"])
        if suspicious:
            raise RuntimeError("snapshot requires credential audit/exclusions: "+", ".join(suspicious))
        manifest["cases"].append({"case_id":config["case_id"],"snapshot":str(snapshot.resolve()),"base_sha":config["base_sha"],"cutoff":config["cutoff"],"snapshot_sha256":summary["snapshot_sha256"],"message_ids":config["ids"],"excluded_prefixes":config["excludes"]})
    write(args.output_dir/"manifest.json",manifest)
    print(json.dumps({"fixture_dir":str(args.output_dir.resolve()),"cases":[c["case_id"] for c in manifest["cases"]]}))

if __name__ == "__main__":
    main()
