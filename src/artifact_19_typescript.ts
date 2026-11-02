import http from 'http';

const PORT = 8080;
const server = http.createServer((req, res) => {
  if (req.url === '/health' && req.method === 'GET') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ status: 'UP', service: 'solana-blockscope', timestamp: Date.now() }));
  } else {
    res.writeHead(404);
    res.end();
  }
});

server.listen(PORT, () => {
  console.log(`[HealthService] Operational health endpoint listening on :${PORT}/health`);
});