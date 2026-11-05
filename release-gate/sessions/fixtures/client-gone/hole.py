"""Takes every connect to port 443 and never answers: a download that goes nowhere, however long its caller waits."""
import socket

server = socket.socket()
server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
server.bind(("127.0.0.1", 443))
server.listen(64)
held = []
while True:
    held.append(server.accept())
