// A busy computer's sockets (Paul, 11-14: "open a few thousand sockets first, as on my laptop"): <pairs> loopback TCP connections
// held open, both ends in this process, which the gate's peer lookup reads past. Prints `ready <sockets>` once all are open, and holds
// them until killed.
//   node busy.js <pairs>
const net = require('net');

const pairs = Number(process.argv[2] || 2000);
const held = [];
const server = net.createServer((socket) => held.push(socket));
server.listen(0, '127.0.0.1', async () => {
  const { port } = server.address();
  for (let at = 0; at < pairs; at++) {
    await new Promise((done, failed) => {
      const socket = net.connect(port, '127.0.0.1', () => done());
      socket.on('error', failed);
      held.push(socket);
    });
  }
  console.log(`ready ${held.length}`);
});
setInterval(() => {}, 1 << 30);
