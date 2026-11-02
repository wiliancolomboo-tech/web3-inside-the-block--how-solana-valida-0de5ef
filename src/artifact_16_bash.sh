cd telemetry
npm install
npm run build

# Configure environment
echo "SOLANA_RPC_ENDPOINT=https://api.mainnet-beta.solana.com" > .env
echo "METRICS_PORT=9090" >> .env
echo "DISCORD_WEBHOOK_URL=https://api.example.com/webhooks/alerts/placeholder" >> .env

# Start with Docker Compose
cd ..
docker-compose up -d --build