#!/usr/bin/env python3
"""Serve Hex's design through the local Frame checkout without editing Frame."""
import argparse
import re
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit, unquote

HEX = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser()
parser.add_argument('--frame', type=Path, default=HEX.parent / 'frame')
parser.add_argument('--port', type=int, default=8776)
args = parser.parse_args()
FRAME = args.frame.resolve()
if not (FRAME / 'index.html').is_file():
    parser.error('a local Frame checkout with index.html is required')

class Handler(SimpleHTTPRequestHandler):
    def translate_path(self, path):
        route = unquote(urlsplit(path).path)
        root = FRAME if route.startswith('/frame/') else HEX
        relative = route.removeprefix('/frame/') if root == FRAME else route.lstrip('/')
        target = (root / relative).resolve()
        if not target.is_relative_to(root):
            return str(HEX / '.invalid-path')
        return str(target)

    def do_POST(self):
        names = {'arrangement.png', 'session.png', 'palette.png', 'theme-palette.png', 'hex.frame.json', 'review.json', 'state-default.png', 'state-playing.png', 'state-session.png', 'state-session-playing.png', 'state-clip-colors.png', 'state-time-selection.png'}
        name = urlsplit(self.path).path.removeprefix('/export/')
        size = int(self.headers.get('Content-Length', '0'))
        if not self.path.startswith('/export/') or name not in names or not 0 < size <= 16000000:
            self.send_error(400)
            return
        payload = self.rfile.read(size)
        (HEX / 'design' / 'frame' / name).write_bytes(payload)
        self.send_response(204)
        self.end_headers()

    def do_GET(self):
        if urlsplit(self.path).path == '/':
            html = (FRAME / 'index.html').read_text()
            html = re.sub(r'  <script src="(?:examples|styleguides)/[^\"]+" defer></script>\n', '', html)
            html = re.sub(r'(src|href)="([^\"]+\.(?:js|css)(?:\?[^\"]*)?)"', r'\1="/frame/\2"', html)
            html = html.replace('</head>', '<script src="/design/frame/load.js?r9" defer></script></head>')
            payload = html.encode()
            self.send_response(200)
            self.send_header('Content-Type', 'text/html; charset=utf-8')
            self.send_header('Content-Length', str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        else:
            super().do_GET()

print(f'hex design: http://127.0.0.1:{args.port}', flush=True)
ThreadingHTTPServer(('127.0.0.1', args.port), Handler).serve_forever()
