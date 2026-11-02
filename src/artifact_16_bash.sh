cd telemetry
npm install
npm run build

# Configure environment
echo "SOLANA_RPC_ENDPOINT=https://api.mainnet-beta.solana.com" > .env
echo "METRICS_PORT=9090" >> .env
echo "DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/your/token" >> .env

# Start with Docker Compose
cd ..
docker-compose up -d --build