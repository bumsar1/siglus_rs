"""Tiny WebSocket client (stdlib only): prints every text message.
usage: python3 ws_client.py [path] [port] [seconds]"""
import base64, os, socket, struct, sys, time
path = sys.argv[1] if len(sys.argv) > 1 else "/"
port = int(sys.argv[2]) if len(sys.argv) > 2 else 6677
secs = float(sys.argv[3]) if len(sys.argv) > 3 else 60
deadline = time.time() + secs
while True:
    try:
        s = socket.create_connection(("127.0.0.1", port), timeout=2); break
    except OSError:
        if time.time() > deadline: sys.exit("no server")
        time.sleep(0.3)
key = base64.b64encode(os.urandom(16)).decode()
s.sendall(f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n".encode())
buf = b""
while b"\r\n\r\n" not in buf: buf += s.recv(1024)
head, buf = buf.split(b"\r\n\r\n", 1)
print("#", head.split(b"\r\n")[0].decode(), flush=True)
def need(n):
    global buf
    while len(buf) < n:
        s.settimeout(max(0.1, deadline - time.time()))
        d = s.recv(65536)
        if not d: raise EOFError
        buf += d
    out, buf = buf[:n], buf[n:]; return out
try:
    while time.time() < deadline:
        b0, b1 = need(2)
        n = b1 & 127
        if n == 126: n = struct.unpack(">H", need(2))[0]
        elif n == 127: n = struct.unpack(">Q", need(8))[0]
        data = need(n)
        if b0 & 15 == 1: print(data.decode(), flush=True)
except (socket.timeout, EOFError):
    pass
