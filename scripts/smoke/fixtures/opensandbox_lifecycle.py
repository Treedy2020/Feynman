"""In-process upstream lifecycle fixture, not a replacement Feynman server."""
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading
from urllib.parse import urlparse
import uuid
from email.parser import BytesParser
from email.policy import default


class State:
    def __init__(self):
        self.records = {}
        self.creates = []
        self.ambiguous_once = False
        self.bad_endpoint = False
        self.bad_owner = False
        self.unavailable = False
        self.port = None
        self.seed_upload = None
        self.command_jsonl = True


@contextmanager
def upstream():
    state = State()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def respond(self, code, value):
            body = json.dumps(value).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if state.unavailable:
                self.respond(503, {"error": "upstream offline"})
                return
            path = urlparse(self.path).path
            if path == "/v1/sandboxes":
                self.respond(200, {"items": list(state.records.values()), "pagination": {"hasNextPage": False}})
                return
            identifier = path.split("/")[3]
            if identifier not in state.records:
                self.respond(404, {})
                return
            if "/endpoints/" in path:
                address = "192.0.2.1:44773" if state.bad_endpoint else f"127.0.0.1:{state.port}/proxy/44773"
                self.respond(200, {"endpoint": address})
                return
            record = dict(state.records[identifier])
            if state.bad_owner:
                record["metadata"] = {"feynman-project": "different-project"}
            self.respond(200, record)

        def do_POST(self):
            path = urlparse(self.path).path
            raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            if path.endswith('/proxy/44772/files/upload'):
                message = BytesParser(policy=default).parsebytes(('Content-Type: '+self.headers['Content-Type']+'\r\nMIME-Version: 1.0\r\n\r\n').encode()+raw)
                parts = {p.get_param('name',header='content-disposition'):p for p in message.iter_parts()}
                metadata = parts.get('metadata')
                if metadata is None or not metadata.get_filename() or metadata.get_content_type()!='application/json':
                    self.respond(400,{'code':'INVALID_FILE_METADATA','message':'metadata file is missing'})
                    return
                meta = json.loads(metadata.get_payload(decode=True))
                if meta['mode'] != 600 or not meta['path'].startswith('/workspace/.seed-'):
                    self.respond(400,{'code':'INVALID_FILE_METADATA'})
                    return
                state.seed_upload = json.loads(parts['file'].get_payload(decode=True))
                self.respond(200,{})
                return
            value = json.loads(raw or '{}')
            if path.endswith('/proxy/44772/command'):
                assert state.seed_upload and value['cwd']=='/workspace' and value['timeout']==60000
                assert value['command'].startswith('python3 -c ')
                seed=state.seed_upload
                manifest={'base_sha':seed['base_sha'],'file_count':len(seed['files']),'cutoff':seed['facts']['cutoff']}
                prefix='' if state.command_jsonl else 'data: '
                body=(prefix+json.dumps({'type':'stdout','text':json.dumps(manifest)+'\n'})+'\n\n'+prefix+json.dumps({'type':'execution_complete'})+'\n\n').encode()
                self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
                return
            if path == "/v1/sandboxes":
                state.creates.append(value)
                identifier = str(uuid.uuid4())
                record = {"id": identifier, "metadata": value["metadata"], "status": {"state": "Running"}, "expiresAt": (datetime.now(timezone.utc) + timedelta(minutes=30)).isoformat()}
                state.records[identifier] = record
                if state.ambiguous_once:
                    state.ambiguous_once = False
                    self.respond(503, {"error": "response lost after creation"})
                else:
                    self.respond(200, record)
                return
            identifier, action = path.split("/")[3:5]
            if identifier not in state.records:
                self.respond(404, {})
                return
            if action == "pause":
                state.records[identifier]["status"]["state"] = "Paused"
            elif action == "resume":
                state.records[identifier]["status"]["state"] = "Running"
            elif action == "renew-expiration":
                state.records[identifier]["expiresAt"] = value["expiresAt"]
            self.respond(200, {})

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    state.port = server.server_port
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{state.port}/", state
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
