"""Count a text's tokens with the served tokenizer, as vLLM's /tokenize does
for a completion-form prompt with add_special_tokens false: one request, the
text's UTF-8 bytes as the body, the count as the reply. Serves on a Unix socket."""
import os, socketserver, sys
from http.server import BaseHTTPRequestHandler
from tokenizers import Tokenizer

tok = Tokenizer.from_file(sys.argv[1])
path = sys.argv[2]

class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers['Content-Length'])
        text = self.rfile.read(n).decode('utf-8')
        count = len(tok.encode(text, add_special_tokens=False).ids)
        body = str(count).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass
    def address_string(self):
        return 'unix'

class Server(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

if os.path.exists(path):
    os.unlink(path)
with Server(path, Handler) as s:
    os.chmod(path, 0o777)
    s.serve_forever()
